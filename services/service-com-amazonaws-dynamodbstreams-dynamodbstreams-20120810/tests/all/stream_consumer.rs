// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DynamoDB Streams consumer loop: `ListStreams`,
//! `DescribeStream`, `GetShardIterator` and `GetRecords`, driven over the wire
//! (`POST /` + `X-Amz-Target: DynamoDBStreams_20120810.<Op>`) through a router with
//! this service mounted.
//!
//! Records get INTO the store through the publishing seam on the service's state
//! (`register_stream` / `append_record` / `roll_shard`), which is what DynamoDB's
//! `CreateTable`(StreamSpecification) / `PutItem` / `UpdateItem` / `DeleteItem` call
//! once they are implemented. Every assertion below reads the result back off the
//! wire, so nothing is proved about the store that a client cannot observe.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;
use smithy_server_runtime::MultiServiceRouter;
use streams::ops::types::{AttributeValue, KeySchemaElement};
use streams::state::{
    AttributeMap, ComAmazonawsDynamodbstreamsDynamodbstreams20120810State as StreamsState,
    OperationType, RecordAppend, RegistrationRejected, StreamRegistration, StreamViewType,
    MAX_LIVE_ITERATORS, MAX_STREAMS_PER_TABLE, STREAM_DISABLE_DELAY,
};

/// A SigV4 credential scope naming `dynamodb`, which is how both DynamoDB and
/// DynamoDB Streams requests are signed. Request authentication is off in these
/// tests, so only the scope's routing signal matters.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/dynamodb/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

const REGION: &str = "us-east-1";

fn build() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(streams::EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("authorization", AUTHORIZATION)
        .header(
            "x-amz-target",
            format!("DynamoDBStreams_20120810.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed =
        serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null);
    (status, parsed)
}

/// A whole-second instant `seconds_ago` before now, so record timestamps compare
/// exactly against an `AT_TIMESTAMP` expressed in epoch seconds.
fn seconds_ago(seconds: u64) -> SystemTime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs();
    UNIX_EPOCH + Duration::from_secs(now - seconds)
}

/// A whole-second instant `seconds` AHEAD of now. Registering the "still
/// enabling" stream at a future instant makes the transient window arbitrarily
/// wide instead of `STREAM_ENABLE_DELAY` wide, so the assertion cannot race a
/// slow dispatch on a loaded machine.
fn seconds_ahead(seconds: u64) -> SystemTime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs();
    UNIX_EPOCH + Duration::from_secs(now + seconds)
}

fn epoch_secs(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the instant is after the Unix epoch")
        .as_secs()
}

fn key_schema() -> Vec<KeySchemaElement> {
    vec![KeySchemaElement {
        attribute_name: "Id".to_string(),
        key_type: "HASH".to_string(),
    }]
}

fn register(
    engine: &engine::InMemoryEngine,
    table_name: &str,
    stream_view_type: StreamViewType,
    at: SystemTime,
) -> String {
    engine
        .state::<StreamsState>()
        .lock()
        .register_stream(
            &StreamRegistration {
                table_name: table_name.to_string(),
                stream_view_type,
                key_schema: key_schema(),
                region: REGION.to_string(),
            },
            at,
        )
        .expect("the registry accepts the registration")
}

fn keys_of(id: &str) -> AttributeMap {
    HashMap::from([("Id".to_string(), AttributeValue::S(id.to_string()))])
}

fn image_of(id: &str, payload: &str) -> AttributeMap {
    HashMap::from([
        ("Id".to_string(), AttributeValue::S(id.to_string())),
        (
            "Payload".to_string(),
            AttributeValue::N(payload.to_string()),
        ),
    ])
}

/// Publish one change onto `stream_arn` and return the sequence number it got.
fn append(
    engine: &engine::InMemoryEngine,
    stream_arn: &str,
    event_name: OperationType,
    id: &str,
    at: SystemTime,
) -> String {
    engine
        .state::<StreamsState>()
        .lock()
        .append_record(
            stream_arn,
            &RecordAppend {
                event_name,
                keys: keys_of(id),
                new_image: Some(image_of(id, "42")),
                old_image: Some(image_of(id, "7")),
            },
            at,
        )
        .expect("the stream is registered and has an open shard")
}

/// The single shard of a freshly registered stream, read back over the wire.
async fn only_shard_id(router: &MultiServiceRouter, stream_arn: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeStream",
        serde_json::json!({ "StreamArn": stream_arn }),
    )
    .await;
    assert_eq!(status, 200, "DescribeStream answered {status}: {body}");
    body["StreamDescription"]["Shards"][0]["ShardId"]
        .as_str()
        .unwrap_or_else(|| panic!("no ShardId in {body}"))
        .to_string()
}

async fn shard_iterator(
    router: &MultiServiceRouter,
    stream_arn: &str,
    shard_id: &str,
    iterator_type: &str,
    extra: serde_json::Value,
) -> (u16, serde_json::Value) {
    let mut body = serde_json::json!({
        "StreamArn": stream_arn,
        "ShardId": shard_id,
        "ShardIteratorType": iterator_type,
    });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    call(router, "GetShardIterator", body).await
}

fn error_code(body: &serde_json::Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

fn sequence_numbers(records: &serde_json::Value) -> Vec<String> {
    records
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|record| {
                    record["dynamodb"]["SequenceNumber"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The consumer loop
// ---------------------------------------------------------------------------

/// A `GetShardIterator` followed by `GetRecords` on each returned
/// `NextShardIterator` delivers every record of the shard exactly once, in append
/// order, whatever the shard holds and whatever page size the client asks for.
#[tokio::test]
async fn the_consumer_loop_delivers_every_record_exactly_once_at_any_page_size() {
    // The rule generalises over how many records the shard holds and how large a
    // page the caller asks for, so the table varies both — including a page larger
    // than the shard and a page of exactly one.
    let cases: [(usize, i64); 7] = [(0, 1), (1, 1), (3, 1), (3, 2), (3, 5), (7, 3), (10, 10)];

    for (record_count, limit) in cases {
        let case = format!("{record_count} records, Limit {limit}");
        let (engine, router) = build();
        let base = seconds_ago(600);
        let stream_arn = register(&engine, "ledger", StreamViewType::NewImage, base);
        let expected: Vec<String> = (0..record_count)
            .map(|index| {
                append(
                    &engine,
                    &stream_arn,
                    OperationType::Insert,
                    &format!("item-{index}"),
                    base + Duration::from_secs(index as u64),
                )
            })
            .collect();

        let shard_id = only_shard_id(&router, &stream_arn).await;
        let (status, body) =
            shard_iterator(&router, &stream_arn, &shard_id, "TRIM_HORIZON", serde_json::json!({}))
                .await;
        assert_eq!(
            status, 200,
            "case {case}: GetShardIterator answered {status}, body {body}"
        );
        let mut iterator = body["ShardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no ShardIterator in {body}"))
            .to_string();

        let mut delivered: Vec<String> = Vec::new();
        // Bounded so a handler that never advances fails instead of hanging.
        for _ in 0..record_count + 2 {
            let (status, body) = call(
                &router,
                "GetRecords",
                serde_json::json!({ "ShardIterator": iterator, "Limit": limit }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: GetRecords answered {status}, body {body}"
            );
            let batch = sequence_numbers(&body["Records"]);
            assert!(
                batch.len() <= usize::try_from(limit).unwrap_or(usize::MAX),
                "case {case}: a batch of {} exceeded Limit {limit}",
                batch.len()
            );
            let next = body["NextShardIterator"].as_str().map(str::to_string);
            assert!(
                next.is_some(),
                "case {case}: an OPEN shard must always hand back a usable \
                 NextShardIterator, got {body}"
            );
            delivered.extend(batch.iter().cloned());
            iterator = next.expect("checked just above");
            if batch.is_empty() {
                break;
            }
        }

        assert_eq!(
            delivered, expected,
            "case {case}: the loop must deliver every record exactly once in append \
             order, got {delivered:?}"
        );
        // Sequence numbers are strictly increasing across the whole shard.
        let as_numbers: Vec<u128> = delivered
            .iter()
            .map(|sequence| {
                sequence
                    .parse::<u128>()
                    .unwrap_or_else(|_| panic!("case {case}: {sequence} is not numeric"))
            })
            .collect();
        assert!(
            as_numbers.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case}: sequence numbers must strictly increase, got {delivered:?}"
        );
        assert!(
            delivered.iter().all(|sequence| (21..=40).contains(&sequence.len())),
            "case {case}: every SequenceNumber must be 21..40 characters, got {delivered:?}"
        );

        // A caught-up reader on an open shard gets an empty list and a still-usable
        // iterator, never an error.
        let (status, body) = call(
            &router,
            "GetRecords",
            serde_json::json!({ "ShardIterator": iterator }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a caught-up GetRecords answered {status}, body {body}"
        );
        assert_eq!(
            body["Records"].as_array().map(Vec::len),
            Some(0),
            "case {case}: a caught-up reader must see an empty Records list, got {body}"
        );
        assert!(
            body["NextShardIterator"].is_string(),
            "case {case}: a caught-up reader must keep a usable NextShardIterator, got {body}"
        );
    }
}

/// A shard iterator names an IMMUTABLE position: replaying the same token re-reads
/// the same records, so a read never changes what a later caller sees.
#[tokio::test]
async fn replaying_one_iterator_returns_the_same_records() {
    let (engine, router) = build();
    let base = seconds_ago(600);
    let stream_arn = register(&engine, "ledger", StreamViewType::KeysOnly, base);
    for index in 0..3 {
        append(
            &engine,
            &stream_arn,
            OperationType::Insert,
            &format!("item-{index}"),
            base + Duration::from_secs(index),
        );
    }
    let shard_id = only_shard_id(&router, &stream_arn).await;
    let (_, body) =
        shard_iterator(&router, &stream_arn, &shard_id, "TRIM_HORIZON", serde_json::json!({}))
            .await;
    let iterator = body["ShardIterator"].as_str().expect("an iterator").to_string();

    let (first_status, first) = call(
        &router,
        "GetRecords",
        serde_json::json!({ "ShardIterator": iterator, "Limit": 2 }),
    )
    .await;
    let (second_status, second) = call(
        &router,
        "GetRecords",
        serde_json::json!({ "ShardIterator": iterator, "Limit": 2 }),
    )
    .await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "replaying an iterator must succeed, got {first_status}/{second_status}"
    );
    assert_eq!(
        sequence_numbers(&first["Records"]),
        sequence_numbers(&second["Records"]),
        "replaying one iterator must return the same records, got {:?} then {:?}",
        sequence_numbers(&first["Records"]),
        sequence_numbers(&second["Records"])
    );
    // eventID is stable across reads and unique per record.
    let ids: Vec<&str> = first["Records"]
        .as_array()
        .expect("records")
        .iter()
        .map(|record| record["eventID"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids.len(), 2, "expected two records, got {first}");
    assert_ne!(ids[0], ids[1], "eventID must be unique per record, got {ids:?}");
    assert_eq!(
        ids,
        second["Records"]
            .as_array()
            .expect("records")
            .iter()
            .map(|record| record["eventID"].as_str().unwrap_or_default())
            .collect::<Vec<&str>>(),
        "eventID must be stable across reads"
    );
}

// ---------------------------------------------------------------------------
// Record shape
// ---------------------------------------------------------------------------

/// `NewImage`/`OldImage` are populated per the stream's `StreamViewType`, for every
/// value the enum declares — not only the `KEYS_ONLY` case the issue cited.
#[tokio::test]
async fn images_are_populated_per_the_streams_view_type() {
    // Catalog source: the modelled `StreamViewType` enum, i.e.
    // `STREAM_VIEW_TYPE_VALUES` in `src/ops/types/shared_types.rs`. All four values.
    let cases: [(StreamViewType, bool, bool); 4] = [
        (StreamViewType::KeysOnly, false, false),
        (StreamViewType::NewImage, true, false),
        (StreamViewType::OldImage, false, true),
        (StreamViewType::NewAndOldImages, true, true),
    ];

    let mut sizes: Vec<(StreamViewType, i64)> = Vec::new();
    for (view_type, wants_new, wants_old) in cases {
        let case = view_type.as_str();
        let (engine, router) = build();
        let base = seconds_ago(600);
        let stream_arn = register(&engine, "ledger", view_type, base);
        append(&engine, &stream_arn, OperationType::Modify, "item-0", base);

        let shard_id = only_shard_id(&router, &stream_arn).await;
        let (_, iterator) =
            shard_iterator(&router, &stream_arn, &shard_id, "TRIM_HORIZON", serde_json::json!({}))
                .await;
        let (status, body) = call(
            &router,
            "GetRecords",
            serde_json::json!({ "ShardIterator": iterator["ShardIterator"] }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: GetRecords answered {status}: {body}");

        let record = &body["Records"][0];
        assert_eq!(
            record["eventSource"].as_str(),
            Some("aws:dynamodb"),
            "case {case}: eventSource must be aws:dynamodb, got {record}"
        );
        assert_eq!(
            record["eventName"].as_str(),
            Some("MODIFY"),
            "case {case}: eventName must be the published OperationType, got {record}"
        );
        assert_eq!(
            record["awsRegion"].as_str(),
            Some(REGION),
            "case {case}: awsRegion must be the stream's region, got {record}"
        );
        assert!(
            record["eventVersion"].is_string(),
            "case {case}: eventVersion must be present, got {record}"
        );
        let stream_record = &record["dynamodb"];
        assert_eq!(
            stream_record["StreamViewType"].as_str(),
            Some(case),
            "case {case}: the record must report the stream's view type, got {stream_record}"
        );
        assert_eq!(
            stream_record["Keys"]["Id"]["S"].as_str(),
            Some("item-0"),
            "case {case}: Keys is populated for every view type, got {stream_record}"
        );
        assert_eq!(
            stream_record["NewImage"].is_object(),
            wants_new,
            "case {case}: NewImage presence must follow the view type, got {stream_record}"
        );
        assert_eq!(
            stream_record["OldImage"].is_object(),
            wants_old,
            "case {case}: OldImage presence must follow the view type, got {stream_record}"
        );
        assert!(
            stream_record["ApproximateCreationDateTime"].is_number(),
            "case {case}: ApproximateCreationDateTime must be present, got {stream_record}"
        );

        let size = stream_record["SizeBytes"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case}: no SizeBytes in {stream_record}"));
        assert!(
            size >= 1,
            "case {case}: SizeBytes is modelled range 1.., got {size}"
        );
        sizes.push((view_type, size));
    }

    // DynamoDB sizes an item as the sum of the lengths of its attribute names and
    // values, so a KEYS_ONLY record of {"Id": {"S": "item-0"}} is 2 + 6 = 8 bytes…
    let keys_only = sizes
        .iter()
        .find(|(view, _)| *view == StreamViewType::KeysOnly)
        .map(|(_, size)| *size);
    assert_eq!(
        keys_only,
        Some(8),
        "a KEYS_ONLY record carrying only Id=item-0 sizes as 8 bytes, got {keys_only:?}"
    );
    // …and carrying more images can only make a record bigger.
    let size_of = |view: StreamViewType| {
        sizes
            .iter()
            .find(|(candidate, _)| *candidate == view)
            .map(|(_, size)| *size)
            .unwrap_or_default()
    };
    assert!(
        size_of(StreamViewType::KeysOnly) < size_of(StreamViewType::NewImage)
            && size_of(StreamViewType::NewImage) < size_of(StreamViewType::NewAndOldImages),
        "SizeBytes must grow with the images the stream captures, got {sizes:?}"
    );
}

/// Every `ShardIteratorType` the enum declares positions the cursor where the model
/// says it does.
#[tokio::test]
async fn every_shard_iterator_type_positions_where_the_model_says() {
    let (engine, router) = build();
    let base = seconds_ago(600);
    let stream_arn = register(&engine, "ledger", StreamViewType::KeysOnly, base);
    let appended: Vec<String> = (0..4)
        .map(|index| {
            append(
                &engine,
                &stream_arn,
                OperationType::Insert,
                &format!("item-{index}"),
                base + Duration::from_secs(index),
            )
        })
        .collect();
    let shard_id = only_shard_id(&router, &stream_arn).await;
    let third_timestamp = epoch_secs(base + Duration::from_secs(2));

    // Catalog source: the modelled `ShardIteratorType` enum, i.e.
    // `SHARD_ITERATOR_TYPE_VALUES`. All five values, with the sequence-number and
    // timestamp forms pointed at the third record.
    let cases: [(&str, serde_json::Value, Vec<String>); 5] = [
        ("TRIM_HORIZON", serde_json::json!({}), appended.clone()),
        ("LATEST", serde_json::json!({}), Vec::new()),
        (
            "AT_SEQUENCE_NUMBER",
            serde_json::json!({ "SequenceNumber": appended[2] }),
            appended[2..].to_vec(),
        ),
        (
            "AFTER_SEQUENCE_NUMBER",
            serde_json::json!({ "SequenceNumber": appended[2] }),
            appended[3..].to_vec(),
        ),
        (
            "AT_TIMESTAMP",
            serde_json::json!({ "Timestamp": third_timestamp }),
            appended[2..].to_vec(),
        ),
    ];

    for (iterator_type, extra, expected) in cases {
        let (status, body) =
            shard_iterator(&router, &stream_arn, &shard_id, iterator_type, extra).await;
        assert_eq!(
            status, 200,
            "case {iterator_type}: GetShardIterator answered {status}: {body}"
        );
        let token = body["ShardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {iterator_type}: no ShardIterator in {body}"));
        assert!(
            (1..=2048).contains(&token.len()),
            "case {iterator_type}: ShardIterator is modelled len 1..2048, got {}",
            token.len()
        );
        // A DynamoDB Streams shard iterator is `<stream ARN>|<format version>|<opaque
        // blob>` — the shape of the `ShardIterator` in the `GetShardIterator` example
        // of the Amazon DynamoDB Streams API Reference — so the STREAM an iterator
        // belongs to is readable off the token (the DynamoDB Streams Kinesis adapter
        // reads it there) while the POSITION it names is not: the shard and the
        // offset stay in the service's own iterator table.
        let (named, blob) = token
            .rsplit_once('|')
            .unwrap_or_else(|| panic!("case {iterator_type}: {token} is not an iterator token"));
        assert_eq!(
            named,
            format!("{stream_arn}|1"),
            "case {iterator_type}: the token must name its stream and format version, got {token}"
        );
        assert!(
            !blob.is_empty() && !blob.contains(&shard_id),
            "case {iterator_type}: the token's blob must be opaque, not an echo of the input: {token}"
        );

        let (status, body) = call(
            &router,
            "GetRecords",
            serde_json::json!({ "ShardIterator": token }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {iterator_type}: GetRecords answered {status}: {body}"
        );
        assert_eq!(
            sequence_numbers(&body["Records"]),
            expected,
            "case {iterator_type}: wrong starting position, got {:?}",
            sequence_numbers(&body["Records"])
        );
    }
}

// ---------------------------------------------------------------------------
// DescribeStream / ListStreams
// ---------------------------------------------------------------------------

/// `DescribeStream` reports the same stream `ListStreams` returned, with the shard
/// topology the publishing seam built: an open shard has no `EndingSequenceNumber`,
/// a rolled-over one does, and the child names its parent.
#[tokio::test]
async fn describe_stream_agrees_with_list_streams_and_reports_the_shard_topology() {
    let (engine, router) = build();
    let base = seconds_ago(600);
    let stream_arn = register(&engine, "ledger", StreamViewType::NewAndOldImages, base);
    let appended: Vec<String> = (0..2)
        .map(|index| {
            append(
                &engine,
                &stream_arn,
                OperationType::Insert,
                &format!("item-{index}"),
                base + Duration::from_secs(index),
            )
        })
        .collect();

    let (status, listed) = call(&router, "ListStreams", serde_json::json!({})).await;
    assert_eq!(status, 200, "ListStreams answered {status}: {listed}");
    let entry = &listed["Streams"][0];
    assert_eq!(
        entry["StreamArn"].as_str(),
        Some(stream_arn.as_str()),
        "ListStreams must return the registered stream, got {listed}"
    );
    assert_eq!(
        entry["TableName"].as_str(),
        Some("ledger"),
        "ListStreams must report the stream's table, got {listed}"
    );

    let (status, described) = call(
        &router,
        "DescribeStream",
        serde_json::json!({ "StreamArn": stream_arn }),
    )
    .await;
    assert_eq!(status, 200, "DescribeStream answered {status}: {described}");
    let description = &described["StreamDescription"];
    assert_eq!(
        (
            description["StreamArn"].as_str(),
            description["TableName"].as_str(),
            description["StreamLabel"].as_str(),
        ),
        (
            entry["StreamArn"].as_str(),
            entry["TableName"].as_str(),
            entry["StreamLabel"].as_str(),
        ),
        "DescribeStream must report what ListStreams returned, got {description}"
    );
    assert_eq!(
        description["StreamStatus"].as_str(),
        Some("ENABLED"),
        "a stream registered 10 minutes ago is ENABLED, got {description}"
    );
    assert_eq!(
        description["StreamViewType"].as_str(),
        Some("NEW_AND_OLD_IMAGES"),
        "DescribeStream must report the registered view type, got {description}"
    );
    assert_eq!(
        description["KeySchema"][0]["AttributeName"].as_str(),
        Some("Id"),
        "DescribeStream must report the table's key schema, got {description}"
    );
    assert_eq!(
        description["KeySchema"][0]["KeyType"].as_str(),
        Some("HASH"),
        "DescribeStream must report the table's key schema, got {description}"
    );
    assert!(
        description["CreationRequestDateTime"].is_number(),
        "CreationRequestDateTime must be present, got {description}"
    );

    let parent = &description["Shards"][0];
    let parent_id = parent["ShardId"].as_str().expect("a ShardId").to_string();
    assert!(
        (28..=65).contains(&parent_id.len()),
        "ShardId is modelled len 28..65, got {} for {parent_id}",
        parent_id.len()
    );
    assert!(
        parent["SequenceNumberRange"]["StartingSequenceNumber"].is_string(),
        "an open shard still reports its StartingSequenceNumber, got {parent}"
    );
    assert!(
        parent["SequenceNumberRange"]["EndingSequenceNumber"].is_null(),
        "EndingSequenceNumber must be ABSENT for an open shard, got {parent}"
    );
    assert!(
        parent["ParentShardId"].is_null(),
        "the first shard of a stream has no parent, got {parent}"
    );

    // Roll the shard over: the parent closes, a child opens on it.
    let child_id = engine
        .state::<StreamsState>()
        .lock()
        .roll_shard(&stream_arn, base + Duration::from_secs(10))
        .expect("the stream has an open shard to roll");

    let (status, described) = call(
        &router,
        "DescribeStream",
        serde_json::json!({ "StreamArn": stream_arn }),
    )
    .await;
    assert_eq!(status, 200, "DescribeStream answered {status}: {described}");
    let shards = described["StreamDescription"]["Shards"]
        .as_array()
        .expect("shards")
        .clone();
    assert_eq!(
        shards.len(),
        2,
        "a rolled stream has the closed parent and the open child, got {shards:?}"
    );
    assert_eq!(
        shards[0]["SequenceNumberRange"]["EndingSequenceNumber"].as_str(),
        appended.last().map(String::as_str),
        "a closed shard ends on its last record's sequence number, got {:?}",
        shards[0]
    );
    assert_eq!(
        shards[1]["ShardId"].as_str(),
        Some(child_id.as_str()),
        "the child shard follows its parent, got {:?}",
        shards[1]
    );
    assert_eq!(
        shards[1]["ParentShardId"].as_str(),
        Some(parent_id.as_str()),
        "the child must name its parent, got {:?}",
        shards[1]
    );
    assert!(
        shards[1]["SequenceNumberRange"]["EndingSequenceNumber"].is_null(),
        "the new child shard is open, got {:?}",
        shards[1]
    );

    // ShardFilter CHILD_SHARDS narrows to the children of one shard.
    let (status, filtered) = call(
        &router,
        "DescribeStream",
        serde_json::json!({
            "StreamArn": stream_arn,
            "ShardFilter": { "Type": "CHILD_SHARDS", "ShardId": parent_id },
        }),
    )
    .await;
    assert_eq!(status, 200, "a filtered DescribeStream answered {status}: {filtered}");
    let filtered_ids: Vec<&str> = filtered["StreamDescription"]["Shards"]
        .as_array()
        .expect("shards")
        .iter()
        .map(|shard| shard["ShardId"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        filtered_ids,
        vec![child_id.as_str()],
        "CHILD_SHARDS must return only the named shard's children, got {filtered_ids:?}"
    );

    // Shards page by Limit / ExclusiveStartShardId, the same way streams do.
    let (status, first_page) = call(
        &router,
        "DescribeStream",
        serde_json::json!({ "StreamArn": stream_arn, "Limit": 1 }),
    )
    .await;
    assert_eq!(status, 200, "a limited DescribeStream answered {status}: {first_page}");
    assert_eq!(
        first_page["StreamDescription"]["Shards"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "Limit 1 must return one shard, got {first_page}"
    );
    assert_eq!(
        first_page["StreamDescription"]["LastEvaluatedShardId"].as_str(),
        Some(parent_id.as_str()),
        "a truncated shard page must report LastEvaluatedShardId, got {first_page}"
    );
    let (status, second_page) = call(
        &router,
        "DescribeStream",
        serde_json::json!({
            "StreamArn": stream_arn,
            "Limit": 1,
            "ExclusiveStartShardId": parent_id,
        }),
    )
    .await;
    assert_eq!(status, 200, "the second shard page answered {status}: {second_page}");
    assert_eq!(
        second_page["StreamDescription"]["Shards"][0]["ShardId"].as_str(),
        Some(child_id.as_str()),
        "ExclusiveStartShardId must resume after the named shard, got {second_page}"
    );
    assert_eq!(
        second_page["StreamDescription"]["LastEvaluatedShardId"].as_str(),
        None,
        "the page that stopped at the end of the shard list offers no cursor, got \
         {second_page}"
    );
    // A client that asks anyway, resuming after the last shard, is served an empty page
    // without a cursor rather than a refusal.
    let (status, past_end) = call(
        &router,
        "DescribeStream",
        serde_json::json!({
            "StreamArn": stream_arn,
            "Limit": 1,
            "ExclusiveStartShardId": child_id,
        }),
    )
    .await;
    assert_eq!(status, 200, "the page past the last shard answered {status}: {past_end}");
    assert_eq!(
        past_end["StreamDescription"]["Shards"].as_array().map(Vec::len),
        Some(0),
        "resuming after the last shard evaluates nothing, got {past_end}"
    );
    assert_eq!(
        past_end["StreamDescription"]["LastEvaluatedShardId"].as_str(),
        None,
        "a page that evaluated no shard offers no cursor, got {past_end}"
    );

    // Draining a CLOSED shard retires the iterator: NextShardIterator goes null.
    let (_, iterator) =
        shard_iterator(&router, &stream_arn, &parent_id, "TRIM_HORIZON", serde_json::json!({}))
            .await;
    let (status, drained) = call(
        &router,
        "GetRecords",
        serde_json::json!({ "ShardIterator": iterator["ShardIterator"] }),
    )
    .await;
    assert_eq!(status, 200, "GetRecords answered {status}: {drained}");
    assert_eq!(
        sequence_numbers(&drained["Records"]),
        appended,
        "a closed shard still delivers its records, got {:?}",
        sequence_numbers(&drained["Records"])
    );
    assert!(
        drained["NextShardIterator"].is_null(),
        "a closed, drained shard must answer NextShardIterator null, got {drained}"
    );
}

/// `ListStreams` pages by `Limit`/`ExclusiveStartStreamArn` and filters by
/// `TableName`; a table with no stream is an empty list, not an error.
#[tokio::test]
async fn list_streams_pages_and_filters_over_the_registered_streams() {
    let (engine, router) = build();
    // Three streams on one table and two on another, each registered at its own
    // instant so each gets its own ARN.
    let mut expected_alpha: Vec<String> = (0..3)
        .map(|index| register(&engine, "alpha", StreamViewType::KeysOnly, seconds_ago(900 - index)))
        .collect();
    let mut expected_beta: Vec<String> = (0..2)
        .map(|index| register(&engine, "beta", StreamViewType::NewImage, seconds_ago(800 - index)))
        .collect();
    expected_alpha.sort();
    expected_beta.sort();
    let mut expected_all: Vec<String> = expected_alpha
        .iter()
        .chain(expected_beta.iter())
        .cloned()
        .collect();
    expected_all.sort();

    // Page sizes that divide the set evenly and unevenly, plus one larger than it.
    for limit in [1_i64, 2, 3, 5, 9] {
        let case = format!("Limit {limit}");
        let mut seen: Vec<String> = Vec::new();
        let mut start: Option<String> = None;
        for _ in 0..expected_all.len() + 2 {
            let mut body = serde_json::json!({ "Limit": limit });
            if let (Some(target), Some(start)) = (body.as_object_mut(), start.as_ref()) {
                target.insert(
                    "ExclusiveStartStreamArn".to_string(),
                    serde_json::Value::String(start.clone()),
                );
            }
            let (status, page) = call(&router, "ListStreams", body).await;
            assert_eq!(status, 200, "case {case}: ListStreams answered {status}: {page}");
            let arns: Vec<String> = page["Streams"]
                .as_array()
                .expect("Streams is always present")
                .iter()
                .map(|entry| entry["StreamArn"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                arns.len() <= usize::try_from(limit).unwrap_or(usize::MAX),
                "case {case}: a page of {} exceeded the Limit",
                arns.len()
            );
            seen.extend(arns.iter().cloned());
            match page["LastEvaluatedStreamArn"].as_str() {
                Some(last) => {
                    assert_eq!(
                        Some(last.to_string()),
                        arns.last().cloned(),
                        "case {case}: LastEvaluatedStreamArn must be the last ARN of the \
                         truncated page, got {page}"
                    );
                    start = Some(last.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, expected_all,
            "case {case}: paging must visit every stream once, in ARN order, got {seen:?}"
        );
    }

    // Filtering by table name, including a table that has no stream at all.
    for (table_name, expected) in [
        ("alpha", expected_alpha.clone()),
        ("beta", expected_beta.clone()),
        ("gamma", Vec::new()),
    ] {
        let (status, page) = call(
            &router,
            "ListStreams",
            serde_json::json!({ "TableName": table_name }),
        )
        .await;
        assert_eq!(status, 200, "case {table_name}: ListStreams answered {status}: {page}");
        let arns: Vec<String> = page["Streams"]
            .as_array()
            .unwrap_or_else(|| panic!("case {table_name}: Streams must be a list, got {page}"))
            .iter()
            .map(|entry| entry["StreamArn"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            arns, expected,
            "case {table_name}: TableName must filter the enumeration, got {arns:?}"
        );
    }
}

/// A stream is `ENABLING` until the asynchronous enable the registration started is
/// due, then `ENABLED`.
#[tokio::test]
async fn a_streams_status_follows_the_clock() {
    let (engine, router) = build();
    // Registered an hour from now: the enable cannot fall due while this test
    // runs, whatever the machine is doing, so `ENABLING` is deterministic.
    let enabling = register(&engine, "fresh", StreamViewType::KeysOnly, seconds_ahead(3600));
    // Registered a minute ago: long past due.
    let enabled = register(&engine, "settled", StreamViewType::KeysOnly, seconds_ago(60));

    for (case, stream_arn, expected) in [
        ("just registered", enabling, "ENABLING"),
        ("registered a minute ago", enabled, "ENABLED"),
    ] {
        let (status, body) = call(
            &router,
            "DescribeStream",
            serde_json::json!({ "StreamArn": stream_arn }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: DescribeStream answered {status}: {body}");
        assert_eq!(
            body["StreamDescription"]["StreamStatus"].as_str(),
            Some(expected),
            "case {case}: wrong StreamStatus, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// An unregistered stream, an unknown shard and a token the service never issued
/// each get the modelled error, not a 200 with an empty body.
#[tokio::test]
async fn unknown_resources_and_tokens_get_the_modelled_errors() {
    let (engine, router) = build();
    let base = seconds_ago(600);
    let known_arn = register(&engine, "ledger", StreamViewType::KeysOnly, base);
    let known_shard = only_shard_id(&router, &known_arn).await;
    // 37 characters, inside the modelled `ShardId` 28..65 bound, so the boundary
    // validator passes it and the handler is the one that refuses it.
    let absent_shard = "shardId-00000000000000000000-deadbeef";
    let absent_arn =
        "arn:aws:dynamodb:us-east-1:000000000000:table/absent/stream/2020-01-01T00:00:00.000";

    let cases: [(&str, &str, serde_json::Value, &str); 7] = [
        (
            "DescribeStream on an unregistered stream",
            "DescribeStream",
            serde_json::json!({ "StreamArn": absent_arn }),
            "ResourceNotFoundException",
        ),
        (
            "GetShardIterator on an unregistered stream",
            "GetShardIterator",
            serde_json::json!({
                "StreamArn": absent_arn,
                "ShardId": absent_shard,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
            "ResourceNotFoundException",
        ),
        (
            "GetShardIterator on an unknown shard of a known stream",
            "GetShardIterator",
            serde_json::json!({
                "StreamArn": known_arn,
                "ShardId": absent_shard,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
            "ResourceNotFoundException",
        ),
        (
            "GetRecords with a fabricated token",
            "GetRecords",
            serde_json::json!({ "ShardIterator": "not-a-token-this-service-ever-issued" }),
            "ValidationException",
        ),
        (
            "GetShardIterator AT_SEQUENCE_NUMBER with no SequenceNumber",
            "GetShardIterator",
            serde_json::json!({
                "StreamArn": known_arn,
                "ShardId": known_shard,
                "ShardIteratorType": "AT_SEQUENCE_NUMBER",
            }),
            "ValidationException",
        ),
        (
            "DescribeStream with a CHILD_SHARDS filter naming no shard",
            "DescribeStream",
            serde_json::json!({
                "StreamArn": known_arn,
                "ShardFilter": { "Type": "CHILD_SHARDS" },
            }),
            "ValidationException",
        ),
        (
            "DescribeStream resuming from a shard the stream does not have",
            "DescribeStream",
            serde_json::json!({
                "StreamArn": known_arn,
                "ExclusiveStartShardId": absent_shard,
            }),
            "ValidationException",
        ),
    ];

    for (case, operation, body, expected) in cases {
        let (status, response) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {response}"
        );
        let code = error_code(&response);
        assert!(
            code.ends_with(expected),
            "case {case}: expected error code {expected}, got {code} ({response})"
        );
    }
}

/// A `ShardIterator` is classified by WHAT IT IS, not by one lookup failing: the
/// token is an opaque digest, so the only thing that makes a value an iterator is
/// that this service minted it.
///
/// `ExpiredIteratorException` tells a client to re-acquire an iterator and retry
/// ("the shard iterator has expired and can no longer be used to retrieve stream
/// records"), which is only true of an iterator that really was one; a value this
/// service never issued names no position and can never work, so it is refused as a
/// `ValidationException` (HTTP 400). The rows below span the class: tokens of every
/// shape a client might send, a live token, and one this service minted whose
/// lifetime has passed.
#[tokio::test]
async fn a_shard_iterator_is_classified_by_whether_this_service_minted_it() {
    let (engine, router) = build();
    let base = seconds_ago(3600);
    let stream_arn = register(&engine, "ledger", StreamViewType::KeysOnly, base);
    append(&engine, &stream_arn, OperationType::Insert, "item-0", base);
    let shard_id = only_shard_id(&router, &stream_arn).await;

    // Minted directly against the store so the issue instant can be chosen; both
    // tokens are of exactly the form `GetShardIterator` hands out.
    let live = engine.state::<StreamsState>().lock().mint_iterator(
        &stream_arn,
        &shard_id,
        0,
        SystemTime::now(),
    );
    let retired = engine.state::<StreamsState>().lock().mint_iterator(
        &stream_arn,
        &shard_id,
        0,
        seconds_ago(16 * 60),
    );
    // The same length as a real token but never issued, so it is as unattributable
    // as any other string a client invents.
    let forged = "f".repeat(live.len());

    // Each row: what the token IS, the token, and the HTTP status plus error code
    // (empty for a success) the rule requires.
    let cases: [(&str, String, u16, &str); 7] = [
        ("an iterator this service just issued", live.clone(), 200, ""),
        (
            "an iterator issued 16 minutes ago",
            retired,
            400,
            "ExpiredIteratorException",
        ),
        (
            "a word that is not a token",
            "not-a-token-this-service-ever-issued".to_string(),
            400,
            "ValidationException",
        ),
        (
            "a token of the right shape that was never issued",
            forged,
            400,
            "ValidationException",
        ),
        (
            "a live token with one character cut off",
            live[..live.len() - 1].to_string(),
            400,
            "ValidationException",
        ),
        (
            "a live token with trailing whitespace",
            format!("{live} "),
            400,
            "ValidationException",
        ),
        (
            "a live token upper-cased",
            live.to_uppercase(),
            400,
            "ValidationException",
        ),
    ];

    for (case, token, expected_status, expected_code) in cases {
        let (status, body) = call(
            &router,
            "GetRecords",
            serde_json::json!({ "ShardIterator": token }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if expected_code.is_empty() {
            assert_eq!(
                body["Records"].as_array().map(Vec::len),
                Some(1),
                "case {case}: a live iterator delivers the record it stands at, got {body}"
            );
        } else {
            let code = error_code(&body);
            assert!(
                code.ends_with(expected_code),
                "case {case}: expected error code {expected_code}, got {code} ({body})"
            );
        }
    }
}

/// A DISABLED stream is drainable for the documented window and then reclaimed:
/// "the corresponding stream on that table goes into the DISABLED state, and the
/// stream is automatically deleted after 24 hours" (the `DeleteTable` doc). The rows
/// straddle that bound as tightly as a live clock allows, the way the retention rows
/// above do.
#[tokio::test]
async fn a_disabled_stream_is_deleted_after_the_documented_day() {
    // The window runs from the instant the stream REACHES `DISABLED`, which is one
    // `STREAM_DISABLE_DELAY` after the disable was requested, so a row that straddles
    // the bound is named relative to that span rather than by a bare second.
    let span = STREAM_DISABLE_DELAY.as_secs();
    for (case, disabled_seconds_ago, still_registered) in [
        ("disabled a minute ago", 60_u64, true),
        ("disabled just inside 24h", 24 * 60 * 60 - span - 10, true),
        ("disabled just past 24h", 24 * 60 * 60 + span + 1, false),
        ("disabled 25h ago", 25 * 60 * 60, false),
    ] {
        let (engine, router) = build();
        let stream_arn = register(
            &engine,
            "ledger",
            StreamViewType::KeysOnly,
            seconds_ago(disabled_seconds_ago + 3600),
        );
        assert!(
            engine
                .state::<StreamsState>()
                .lock()
                .disable_stream(&stream_arn, seconds_ago(disabled_seconds_ago)),
            "case {case}: the registered stream must be disabled"
        );

        let (status, body) = call(&router, "ListStreams", serde_json::json!({})).await;
        assert_eq!(status, 200, "case {case}: ListStreams answered {status}: {body}");
        let listed = body["Streams"].as_array().map_or(0, Vec::len);
        assert_eq!(
            listed,
            usize::from(still_registered),
            "case {case}: wrong number of streams listed, got {body}"
        );

        let (status, body) = call(
            &router,
            "DescribeStream",
            serde_json::json!({ "StreamArn": stream_arn }),
        )
        .await;
        if still_registered {
            assert_eq!(status, 200, "case {case}: DescribeStream answered {status}: {body}");
            assert_eq!(
                body["StreamDescription"]["StreamStatus"].as_str(),
                Some("DISABLED"),
                "case {case}: a stream disabled in the past is DISABLED, got {body}"
            );
        } else {
            assert_eq!(status, 400, "case {case}: DescribeStream answered {status}: {body}");
            assert!(
                error_code(&body).ends_with("ResourceNotFoundException"),
                "case {case}: a deleted stream is not found, got {body}"
            );
        }
    }
}

/// The registry holds at most [`MAX_STREAMS_PER_TABLE`] streams for one table, so a
/// caller that turns a stream off and on again cannot grow it without limit: past
/// the cap the registration is REFUSED and the control plane answers its own
/// `LimitExceededException`.
#[test]
fn the_registry_refuses_more_streams_than_a_table_may_hold() {
    let engine = engine::InMemoryEngine::new();
    let registration = StreamRegistration {
        table_name: "ledger".to_string(),
        stream_view_type: StreamViewType::KeysOnly,
        key_schema: key_schema(),
        region: REGION.to_string(),
    };
    let state = engine.state::<StreamsState>();
    let mut store = state.lock();

    // Each registration is disabled at once, so the next one cannot be the
    // idempotent hit and has to take a new label — exactly the enable/disable cycle
    // a caller drives through UpdateTable.
    let base = SystemTime::now();
    for index in 0..MAX_STREAMS_PER_TABLE {
        let arn = store
            .register_stream(&registration, base)
            .unwrap_or_else(|rejected| {
                panic!("case stream {index}: the registry must accept it, got {rejected}")
            });
        store.disable_stream(&arn, base);
    }
    assert_eq!(
        store.streams().len(),
        MAX_STREAMS_PER_TABLE,
        "case the cap: every accepted registration is one stream"
    );

    let rejected = store
        .register_stream(&registration, base)
        .expect_err("case past the cap: the registration must be refused");
    assert_eq!(
        rejected,
        RegistrationRejected::TooManyStreamsForTable {
            table_name: "ledger".to_string(),
            limit: MAX_STREAMS_PER_TABLE,
        },
        "case past the cap: wrong refusal, got {rejected}"
    );

    // A DIFFERENT table is unaffected: the cap is per table, and it is read off
    // that table's own ARN prefix.
    let other = StreamRegistration {
        table_name: "payments".to_string(),
        ..registration.clone()
    };
    store
        .register_stream(&other, base)
        .expect("case another table: its own first stream is accepted");
}

/// The LIVE iterator table is capped too: a client that keeps asking for
/// iterators cannot grow the store without limit, and the token the cap evicts
/// answers as the expired iterator it now is rather than as a value that was never
/// one.
#[tokio::test]
async fn the_live_iterator_table_is_capped_and_evicts_the_oldest() {
    let (engine, router) = build();
    let base = seconds_ago(3600);
    let stream_arn = register(&engine, "ledger", StreamViewType::KeysOnly, base);
    append(&engine, &stream_arn, OperationType::Insert, "item-0", base);
    let shard_id = only_shard_id(&router, &stream_arn).await;

    // Mint past the cap, all at the SAME instant, so nothing is retired by the
    // clock and only the cap can be what evicts a token.
    let minted_at = SystemTime::now();
    let state = engine.state::<StreamsState>();
    let mut first = String::new();
    let mut last = String::new();
    for index in 0..=MAX_LIVE_ITERATORS {
        let token = state
            .lock()
            .mint_iterator(&stream_arn, &shard_id, 0, minted_at);
        if index == 0 {
            first = token.clone();
        }
        last = token;
    }
    assert!(
        state.lock().live_iterator_count() <= MAX_LIVE_ITERATORS,
        "case past the cap: the live table must stay bounded, got {}",
        state.lock().live_iterator_count()
    );

    // Each row: which token, and the status plus error code the rule requires.
    for (case, token, expected_status, expected_code) in [
        (
            "the oldest iterator, evicted by the cap",
            first,
            400,
            "ExpiredIteratorException",
        ),
        ("the newest iterator", last, 200, ""),
    ] {
        let (status, body) = call(
            &router,
            "GetRecords",
            serde_json::json!({ "ShardIterator": token }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} with {body}"
        );
        if !expected_code.is_empty() {
            let code = error_code(&body);
            assert!(
                code.ends_with(expected_code),
                "case {case}: expected error code {expected_code}, got {code} ({body})"
            );
        }
    }
}

/// The clock retires a shard iterator 15 minutes after it is issued and trims a
/// record 24 hours after it is written, whether or not anybody called.
#[tokio::test]
async fn the_clock_retires_iterators_and_trims_records() {
    // Each row: how long ago the iterator was issued, and whether it still works.
    // The published lifetime is 15 minutes, so the rows straddle that bound as tightly
    // as a live clock allows: `seconds_ago` FLOORS the clock to a whole second, so the
    // instant it returns is up to one second older than the row names, and the handler
    // stamps its own `now` after the call. An "inside" row therefore keeps a margin
    // wider than that drift; an "outside" row can only drift further outside.
    let cases: [(u64, bool); 4] = [
        (0, true),
        (14 * 60, true),
        (15 * 60 - 10, true),
        (15 * 60 + 1, false),
    ];

    for (issued_seconds_ago, still_usable) in cases {
        let case = format!("iterator issued {issued_seconds_ago}s ago");
        let (engine, router) = build();
        let base = seconds_ago(3600);
        let stream_arn = register(&engine, "ledger", StreamViewType::KeysOnly, base);
        append(&engine, &stream_arn, OperationType::Insert, "item-0", base);
        let shard_id = only_shard_id(&router, &stream_arn).await;
        // Minted directly against the store so the issue instant can be in the past;
        // `GetShardIterator` always stamps it with now.
        let token = engine.state::<StreamsState>().lock().mint_iterator(
            &stream_arn,
            &shard_id,
            0,
            seconds_ago(issued_seconds_ago),
        );

        let (status, body) = call(
            &router,
            "GetRecords",
            serde_json::json!({ "ShardIterator": token }),
        )
        .await;
        if still_usable {
            assert_eq!(status, 200, "case {case}: expected 200, got {status} with {body}");
            assert_eq!(
                body["Records"].as_array().map(Vec::len),
                Some(1),
                "case {case}: the record must still be delivered, got {body}"
            );
        } else {
            assert_eq!(status, 400, "case {case}: expected 400, got {status} with {body}");
            assert!(
                error_code(&body).ends_with("ExpiredIteratorException"),
                "case {case}: expected ExpiredIteratorException, got {body}"
            );
        }
    }

    // A record older than the 24 hour retention limit is trimmed, so an iterator
    // standing at the old trim horizon reads past the oldest retained record. The rows
    // straddle the published 24 hour bound as tightly as a live clock allows — with the
    // same margin the iterator rows above explain, since `seconds_ago` floors the clock
    // and the handler stamps `now` after the write.
    for (case, record_age_seconds, expect_trimmed) in [
        ("23h old record", 23 * 60 * 60, false),
        ("just inside 24h", 24 * 60 * 60 - 10, false),
        ("just past 24h", 24 * 60 * 60 + 1, true),
        ("25h old record", 25 * 60 * 60, true),
    ] {
        let (engine, router) = build();
        let written_at = seconds_ago(record_age_seconds);
        let stream_arn = register(&engine, "ledger", StreamViewType::KeysOnly, written_at);
        append(
            &engine,
            &stream_arn,
            OperationType::Insert,
            "item-0",
            written_at,
        );
        let shard_id = only_shard_id(&router, &stream_arn).await;
        let token = engine
            .state::<StreamsState>()
            .lock()
            .mint_iterator(&stream_arn, &shard_id, 0, SystemTime::now());

        let (status, body) = call(
            &router,
            "GetRecords",
            serde_json::json!({ "ShardIterator": token }),
        )
        .await;
        if expect_trimmed {
            assert_eq!(status, 400, "case {case}: expected 400, got {status} with {body}");
            assert!(
                error_code(&body).ends_with("TrimmedDataAccessException"),
                "case {case}: expected TrimmedDataAccessException, got {body}"
            );
        } else {
            assert_eq!(status, 200, "case {case}: expected 200, got {status} with {body}");
            assert_eq!(
                body["Records"].as_array().map(Vec::len),
                Some(1),
                "case {case}: a record inside the retention window is delivered, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The publishing boundary
// ---------------------------------------------------------------------------

/// `GetRecords` returns "a maximum of 1 MB of data or 1000 stream records,
/// whichever comes first" (model: `GetRecords` doc, the source of
/// `streams::state::MAX_GET_RECORDS_BYTES`), so a page stops on the BYTE budget
/// even when `Limit` would allow more records — and a page is never empty while
/// records are waiting.
#[tokio::test]
async fn get_records_stops_at_the_documented_one_megabyte_page() {
    let budget = streams::state::MAX_GET_RECORDS_BYTES;
    // Each row: how many bytes of payload one record carries, and how many
    // records the first page may therefore hold out of the three written.
    let cases: [(i64, usize); 3] = [
        // Three records that together fit: one page takes all three.
        (budget / 8, 3),
        // Two fit, the third does not.
        (budget / 3, 2),
        // One record larger than the whole budget still goes out alone.
        (budget + 1, 1),
    ];

    for (payload_bytes, expected_first_page) in cases {
        let case = format!("{payload_bytes} byte payloads");
        let (engine, router) = build();
        let at = seconds_ago(60);
        let stream_arn = register(&engine, "bulky", StreamViewType::NewImage, at);
        let payload = "x".repeat(usize::try_from(payload_bytes).expect("a sane payload size"));
        for index in 0..3 {
            let id = format!("item-{index}");
            engine
                .state::<StreamsState>()
                .lock()
                .append_record(
                    &stream_arn,
                    &RecordAppend {
                        event_name: OperationType::Insert,
                        keys: keys_of(&id),
                        new_image: Some(HashMap::from([(
                            "Payload".to_string(),
                            AttributeValue::S(payload.clone()),
                        )])),
                        old_image: None,
                    },
                    at,
                )
                .expect("the stream is registered and has an open shard");
        }

        let shard_id = only_shard_id(&router, &stream_arn).await;
        let (status, iterator) = shard_iterator(
            &router,
            &stream_arn,
            &shard_id,
            "TRIM_HORIZON",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, 200, "case {case}: GetShardIterator answered {status}: {iterator}");

        // `Limit` allows all three, so only the byte budget can shorten the page.
        let (status, body) = call(
            &router,
            "GetRecords",
            serde_json::json!({ "ShardIterator": iterator["ShardIterator"], "Limit": 3 }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: GetRecords answered {status}: {body}");
        assert_eq!(
            body["Records"].as_array().map_or(0, Vec::len),
            expected_first_page,
            "case {case}: wrong first-page size, got {} records",
            body["Records"].as_array().map_or(0, Vec::len)
        );

        // Every record is still delivered, just over more pages: drain the rest.
        let mut delivered = body["Records"].as_array().map_or(0, Vec::len);
        let mut token = body["NextShardIterator"].clone();
        while delivered < 3 {
            let (status, page) = call(
                &router,
                "GetRecords",
                serde_json::json!({ "ShardIterator": token, "Limit": 3 }),
            )
            .await;
            assert_eq!(status, 200, "case {case}: GetRecords answered {status}: {page}");
            let count = page["Records"].as_array().map_or(0, Vec::len);
            assert!(
                count > 0,
                "case {case}: a page must not be empty while records are waiting, got {page}"
            );
            delivered += count;
            token = page["NextShardIterator"].clone();
        }
        assert_eq!(
            delivered, 3,
            "case {case}: every record must be delivered across the pages, got {delivered}"
        );
    }
}

/// An item image nested `depth` levels deep: `{"Deep": {"Deep": ... "leaf"}}`.
/// `depth == 1` is a plain scalar attribute.
fn nested_image(depth: usize) -> AttributeMap {
    let mut value = AttributeValue::S("leaf".to_string());
    for _ in 1..depth {
        value = AttributeValue::M(HashMap::from([("Deep".to_string(), value)]));
    }
    HashMap::from([("Deep".to_string(), value)])
}

/// The append seam accepts an image up to DynamoDB's published 32-level nesting
/// limit and REFUSES anything deeper, so no walk over a stored image (sizing,
/// serialization) is ever driven by a caller-chosen depth. Bound source:
/// `streams::state::MAX_ATTRIBUTE_NESTING_DEPTH`, which cites DynamoDB's
/// "Attribute nesting is limited to 32 levels deep" quota.
#[tokio::test]
async fn an_image_nesting_past_the_published_limit_is_refused_at_the_seam() {
    let limit = streams::state::MAX_ATTRIBUTE_NESTING_DEPTH;
    // Each published bound and one past it, plus the shallow cases and a depth far
    // past the limit (which a recursive walk would not survive).
    let cases: [(usize, bool); 7] = [
        (1, true),
        (2, true),
        (limit - 1, true),
        (limit, true),
        (limit + 1, false),
        (limit * 4, false),
        (limit * 32, false),
    ];

    for (depth, accepted) in cases {
        let case = format!("image nested {depth} deep");
        let (engine, router) = build();
        let at = seconds_ago(60);
        let stream_arn = register(&engine, "deep", StreamViewType::NewImage, at);

        let outcome = engine.state::<StreamsState>().lock().append_record(
            &stream_arn,
            &RecordAppend {
                event_name: OperationType::Insert,
                keys: keys_of("item-0"),
                new_image: Some(nested_image(depth)),
                old_image: None,
            },
            at,
        );

        assert_eq!(
            outcome.is_ok(),
            accepted,
            "case {case}: the seam must {} this image, got {outcome:?}",
            if accepted { "accept" } else { "refuse" }
        );
        if !accepted {
            assert_eq!(
                outcome,
                Err(streams::state::AppendRejected::NestingTooDeep { depth, limit }),
                "case {case}: wrong refusal reason"
            );
        }

        // Whatever the seam decided, a consumer sees exactly what was stored: the
        // record on an accepted image, nothing at all on a refused one.
        let shard_id = only_shard_id(&router, &stream_arn).await;
        let (status, iterator) =
            shard_iterator(&router, &stream_arn, &shard_id, "TRIM_HORIZON", serde_json::json!({}))
                .await;
        assert_eq!(status, 200, "case {case}: GetShardIterator answered {status}: {iterator}");
        let (status, body) = call(
            &router,
            "GetRecords",
            serde_json::json!({ "ShardIterator": iterator["ShardIterator"] }),
        )
        .await;
        assert_eq!(status, 200, "case {case}: GetRecords answered {status}: {body}");
        let delivered = body["Records"].as_array().map_or(0, Vec::len);
        assert_eq!(
            delivered,
            usize::from(accepted),
            "case {case}: wrong number of records delivered, got {body}"
        );
        if accepted {
            // `SizeBytes` has `range min 1`, and the sizer walked the whole image.
            let size_bytes = body["Records"][0]["dynamodb"]["SizeBytes"].as_i64();
            assert!(
                size_bytes.is_some_and(|size| size >= 1),
                "case {case}: SizeBytes must be at least 1, got {body}"
            );
        }
    }
}
