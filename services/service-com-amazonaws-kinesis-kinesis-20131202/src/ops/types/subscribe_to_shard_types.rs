// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `subscribe_to_shard` operation.
//!
//! Hand-authored rather than generated: `SubscribeToShard`'s output member `EventStream`
//! targets a `@streaming` union, so the op-stub generator emitted neither the operation nor
//! its shapes (`src/ops/SKIPPED_STUBS.md`). Two groups:
//!
//! * the REQUEST shapes, declared in full so the framework's own deserializer fills them from
//!   the awsJson body and `constraints::validate` checks them against the model's `@length`
//!   / `@pattern` / enum traits — the request half is judged by the model, not by hand; and
//! * the event structure the output union carries (`SubscribeToShardEvent`), whose own
//!   members (`Record`, `ChildShard`) are already declared beside the other reader of a
//!   shard, so only this one is added here. The union itself is never constructed: an event
//!   is framed by its member name, so only the member's target shape needs a Rust type.
//!
//! `SubscribeToShardOutput` has no shape here: its only member is the `@streaming` payload,
//! so there are no modelled response headers to serialize.
//!
//! Each structure is declared exactly once across this service's `types/` modules, and
//! `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesis-kinesis-20131202.json");

/// `SubscribeToShardInput`: which consumer subscribes, to which shard of the stream its
/// `ConsumerARN` extends, and where in that shard the subscription starts.
#[smithy_macros::smithy(shape = "com.amazonaws.kinesis#SubscribeToShardInput")]
#[derive(Debug, Clone, Default)]
pub struct SubscribeToShardInput {
    pub consumer_arn: String,
    pub stream_id: Option<String>,
    pub shard_id: String,
    pub starting_position: StartingPosition,
    pub dry_run: Option<bool>,
}

/// `StartingPosition`: "The starting position in the data stream from which to start
/// streaming" — the same five `ShardIteratorType` positions `GetShardIterator` takes, with
/// the sequence number or the instant the three that take one read.
#[smithy_macros::smithy(shape = "com.amazonaws.kinesis#StartingPosition")]
#[derive(Debug, Clone, Default)]
pub struct StartingPosition {
    pub r#type: String,
    pub sequence_number: Option<String>,
    pub timestamp: Option<::aws_smithy_types::DateTime>,
}

/// The `SubscribeToShardEvent` member of `SubscribeToShardEventStream`: "After you call
/// SubscribeToShard, Kinesis Data Streams sends events of this type over an HTTP/2 connection
/// to your consumer."
#[smithy_macros::smithy(shape = "com.amazonaws.kinesis#SubscribeToShardEvent")]
#[derive(Debug, Clone, Default)]
pub struct SubscribeToShardEvent {
    pub records: Vec<Record>,
    pub continuation_sequence_number: String,
    pub millis_behind_latest: i64,
    pub child_shards: Option<Vec<ChildShard>>,
}
