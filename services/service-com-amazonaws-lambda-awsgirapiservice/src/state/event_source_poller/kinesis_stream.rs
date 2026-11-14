// SPDX-License-Identifier: CC0-1.0

//! How an event source mapping reads an Amazon KINESIS DATA STREAM: how that stream's
//! shards are listed, how one batch is taken off a shard at the position the mapping
//! carries, and what one record is handed to the function as.
//!
//! Everything a mapping does with the batch once it has it — metering its age, invoking
//! the function, checkpointing past what was processed — is the one rule for every shard
//! source and lives in [`super`]. This module is only the Kinesis half of the two
//! questions that differ per source, the other half being
//! [`super::dynamodb_stream`].
//!
//! The stream is read through the Kinesis surface of this same emulator (`ListShards`,
//! `GetShardIterator`, `GetRecords`) over the in-process callback, so a record is consumed
//! only through the data plane a client would consume it through.

use ::serde_json::{json, Value};

use super::{Clients, ShardRecord, ShardSourcedMapping, MAX_SHARD_PAGES};
use crate::state::ShardCursor;

/// The `eventSource` a Kinesis record envelope reports, and the `eventName` each record
/// carries, as the Kinesis event Lambda hands a function spells them.
const KINESIS_EVENT_SOURCE: &str = "aws:kinesis";
/// See [`KINESIS_EVENT_SOURCE`].
const KINESIS_EVENT_NAME: &str = "aws:kinesis:record";
/// The schema version of the `kinesis` member of one record envelope.
const KINESIS_SCHEMA_VERSION: &str = "1.0";
/// The version of the record envelope itself.
const KINESIS_EVENT_VERSION: &str = "1.0";

/// Every open and closed shard of the mapped stream, in the order the stream reports them.
pub(super) async fn shard_ids(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &ShardSourcedMapping,
) -> Result<Vec<String>, String> {
    let mut shards: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..MAX_SHARD_PAGES {
        let mut request = clients.kinesis(callback, &run.region).list_shards();
        // "NextToken" and the stream the first page named are mutually exclusive: a
        // continuation already carries the stream it is a continuation of.
        request = match next_token.take() {
            Some(token) => request.next_token(token),
            None => request.stream_arn(&run.stream_arn),
        };
        let answer = request.send().await.map_err(|error| {
            format!(
                "the shards of {} could not be listed ({})",
                run.stream_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
        shards.extend(
            answer
                .shards()
                .iter()
                .map(|shard| shard.shard_id().to_string()),
        );
        match answer.next_token() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    Ok(shards)
}

/// One batch off `shard` at `cursor`, each record already rendered as the envelope Lambda
/// hands a Kinesis record over as; `None` for a shard with no position left to read from.
pub(super) async fn read_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &ShardSourcedMapping,
    shard: &str,
    cursor: &ShardCursor,
) -> Result<Option<Vec<ShardRecord>>, String> {
    let Some(iterator) = shard_iterator(callback, clients, run, shard, cursor).await? else {
        return Ok(None);
    };
    let records = records_at(callback, clients, run, &iterator).await?;
    Ok(Some(
        records
            .iter()
            .map(|record| ShardRecord {
                sequence_number: record.sequence_number().to_string(),
                arrived_at: record.approximate_arrival_timestamp().copied(),
                envelope: record_envelope(record, shard, run),
            })
            .collect(),
    ))
}

/// The iterator that reads `shard` from `cursor`, or `None` for a shard that has no
/// position left to read from.
async fn shard_iterator(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &ShardSourcedMapping,
    shard: &str,
    cursor: &ShardCursor,
) -> Result<Option<String>, String> {
    use ::aws_sdk_kinesis::types::ShardIteratorType;

    let request = clients
        .kinesis(callback, &run.region)
        .get_shard_iterator()
        .stream_arn(&run.stream_arn)
        .shard_id(shard);
    let request = match cursor {
        ShardCursor::TrimHorizon => request.shard_iterator_type(ShardIteratorType::TrimHorizon),
        ShardCursor::AtTimestamp(instant) => request
            .shard_iterator_type(ShardIteratorType::AtTimestamp)
            .timestamp(::aws_sdk_kinesis::primitives::DateTime::from(*instant)),
        ShardCursor::AfterSequenceNumber(sequence_number) => request
            .shard_iterator_type(ShardIteratorType::AfterSequenceNumber)
            .starting_sequence_number(sequence_number),
    };
    let answer = request.send().await.map_err(|error| {
        format!(
            "no iterator could be taken on {shard} of {} ({})",
            run.stream_arn,
            ::aws_smithy_types::error::display::DisplayErrorContext(&error)
        )
    })?;
    Ok(answer.shard_iterator().map(str::to_string))
}

/// The records one batch holds: what the shard has at `iterator`, up to the mapping's
/// batch size.
async fn records_at(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &ShardSourcedMapping,
    iterator: &str,
) -> Result<Vec<::aws_sdk_kinesis::types::Record>, String> {
    let answer = clients
        .kinesis(callback, &run.region)
        .get_records()
        .shard_iterator(iterator)
        .limit(run.batch_size)
        .send()
        .await
        .map_err(|error| {
            format!(
                "a batch could not be read from {} ({})",
                run.stream_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    Ok(answer.records)
}

/// One record as Lambda hands a Kinesis record to a function: the record's own data,
/// partition key, sequence number and arrival instant, under the stream it came from.
fn record_envelope(
    record: &::aws_sdk_kinesis::types::Record,
    shard: &str,
    run: &ShardSourcedMapping,
) -> Value {
    let sequence_number = record.sequence_number();
    let mut kinesis = json!({
        "kinesisSchemaVersion": KINESIS_SCHEMA_VERSION,
        "partitionKey": record.partition_key(),
        "sequenceNumber": sequence_number,
        // The record's data travels base64-encoded, as the event a function receives
        // carries it.
        "data": ::aws_smithy_types::base64::encode(record.data().as_ref()),
    });
    if let Some(arrived_at) = record.approximate_arrival_timestamp() {
        kinesis["approximateArrivalTimestamp"] = json!(arrived_at.as_secs_f64());
    }
    let mut envelope = json!({
        "kinesis": kinesis,
        "eventSource": KINESIS_EVENT_SOURCE,
        "eventVersion": KINESIS_EVENT_VERSION,
        // The record is identified by the shard it came from and its own sequence number
        // within that shard.
        "eventID": format!("{shard}:{sequence_number}"),
        "eventName": KINESIS_EVENT_NAME,
        "awsRegion": run.region,
        "eventSourceARN": run.stream_arn,
    });
    if let Some(identity) = run.invoke_identity_arn.as_deref() {
        envelope["invokeIdentityArn"] = json!(identity);
    }
    envelope
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{record_envelope, ShardSourcedMapping};
    use crate::state::event_source_poller::MAX_RECORDS_PER_BATCH;
    use crate::state::{ShardCursor, ShardSource};

    /// A mapping configuration the envelope rows below are built against.
    fn run() -> ShardSourcedMapping {
        ShardSourcedMapping {
            source: ShardSource::Kinesis,
            stream_arn: "arn:aws:kinesis:us-east-1:000000000000:stream/orders".to_string(),
            region: "us-east-1".to_string(),
            function_arn: "arn:aws:lambda:us-east-1:000000000000:function:consume".to_string(),
            function_name: "consume".to_string(),
            invoke_identity_arn: Some("arn:aws:iam::000000000000:role/consume".to_string()),
            batch_size: MAX_RECORDS_PER_BATCH,
            reports_batch_item_failures: false,
            attempts_allowed: None,
            cursors: std::collections::BTreeMap::new(),
            initial_cursor: ShardCursor::TrimHorizon,
        }
    }

    /// One record as the Kinesis data plane answers it.
    fn record(
        sequence_number: &str,
        data: &[u8],
        arrived_at: Option<SystemTime>,
    ) -> ::aws_sdk_kinesis::types::Record {
        let mut builder = ::aws_sdk_kinesis::types::Record::builder()
            .sequence_number(sequence_number)
            .partition_key("orders")
            .data(::aws_sdk_kinesis::primitives::Blob::new(data.to_vec()));
        if let Some(arrived_at) = arrived_at {
            builder = builder.approximate_arrival_timestamp(
                ::aws_sdk_kinesis::primitives::DateTime::from(arrived_at),
            );
        }
        builder.build().expect("the record is well formed")
    }

    /// RULE: each record of a batch is handed over as its OWN envelope, carrying that
    /// record's own data base64-encoded, its partition key, its sequence number and the
    /// stream it came from.
    ///
    /// The rows differ along what the rule generalises over: what the record's data is.
    #[test]
    fn a_record_is_handed_over_carrying_its_own_data() {
        let run = run();
        let arrived_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        // (case, the record's data)
        let rows: Vec<(&str, &[u8])> = vec![
            ("one-word", b"first"),
            ("a-longer-payload", b"{\"order\":\"abc\"}"),
            ("an-empty-payload", b""),
        ];
        for (case, data) in rows {
            let sequence_number = "49590338271490256608559692538361571095921575989136588898";
            let record = record(sequence_number, data, Some(arrived_at));
            let envelope = record_envelope(&record, "shardId-000000000000", &run);
            let encoded = ::aws_smithy_types::base64::encode(data);
            assert_eq!(
                envelope["kinesis"]["data"],
                ::serde_json::json!(encoded),
                "case {case:?}: the record must carry its own data base64-encoded, got \
                 {envelope:?}"
            );
            assert_eq!(
                envelope["eventSourceARN"],
                ::serde_json::json!(run.stream_arn),
                "case {case:?}: the record must name the stream it came from, got {envelope:?}"
            );
            assert_eq!(
                envelope["kinesis"]["sequenceNumber"],
                ::serde_json::json!(sequence_number),
                "case {case:?}: the record must carry its own sequence number, got {envelope:?}"
            );
            assert_eq!(
                envelope["eventID"],
                ::serde_json::json!(format!("shardId-000000000000:{sequence_number}")),
                "case {case:?}: the record must be identified by its shard and sequence \
                 number, got {envelope:?}"
            );
        }
    }
}
