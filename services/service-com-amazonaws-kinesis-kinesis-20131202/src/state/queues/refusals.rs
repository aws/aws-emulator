// SPDX-License-Identifier: CC0-1.0

//! How every refusal a queue operation states is WORDED, and the values a queue is addressed
//! and identified by: one sentence per rule, built from the request and this account, so two
//! operations never word one refusal differently.

use super::*;

/// How long a record is in flight for a queue whose visibility timeout is
/// `visibility_timeout_seconds`.
pub fn visibility_timeout(visibility_timeout_seconds: i32) -> Duration {
    Duration::from_secs(u64::try_from(visibility_timeout_seconds).unwrap_or_default())
}

/// The resource identifier the `QueueARN` `identifier` carries — the segment after
/// `queue/`, which is what names the queue in a refusal, since "the resource identifier in
/// the ARN is generated when the queue is created and is not derived from QueueName"
/// (`QueueDescription$QueueARN`) and the ARN is the only identifier a request addresses a
/// queue by.
pub fn queue_resource_in(identifier: &str) -> &str {
    identifier
        .rsplit_once('/')
        .map_or(identifier, |(_, resource)| resource)
}

/// How an operation states that this account and region hold no queue under the ARN the
/// request named, worded the way this service words that error class for the other kinds of
/// resource it holds ([`channel_not_found_message`]). Built FROM the request and the account
/// serving it.
pub fn queue_not_found_message(identifier: &str) -> String {
    format!(
        "Queue {} under account {} not found.",
        queue_resource_in(identifier),
        ::engine::emulator_account_id()
    )
}

/// How a create refuses a name this account and region already hold a queue under: "The name
/// is scoped to the Amazon Web Services account and Region used by the request"
/// (`CreateQueueInput$QueueName`), which is the modelled "resource is not available for this
/// operation" refusal. Built FROM the request and this account.
pub fn queue_already_exists_message(queue_name: &str) -> String {
    format!(
        "Queue {queue_name} under account {} already exists.",
        ::engine::emulator_account_id()
    )
}

/// How an operation that needs a ready queue refuses one that is not ready yet: the rule
/// `ResourceInUseException` publishes ("The resource is not available for this operation. For
/// successful operation, the resource must be in the ACTIVE state"), worded the way this
/// service words that refusal for its other resources ([`channel_not_active_message`]) —
/// the queue named by the `QueueName` it is stored under, the account that holds it, and the
/// state it is in INSTEAD of `ACTIVE`.
pub fn queue_not_active_message(queue_name: &str, status: &str) -> String {
    format!(
        "Queue {queue_name} under account {} not {QUEUE_ACTIVE}, instead in state {status}.",
        ::engine::emulator_account_id()
    )
}

/// How a receive refuses a queue that is not delivering records: "A queue delivers records
/// while it is in the ACTIVE state" (`ReceiveQueueRecords`), and "A queue in the DELETING
/// state does not deliver records" (`DeleteQueue`). Built FROM the stored record, so the queue
/// and the state it is in name it.
pub fn queue_not_delivering_message(queue_name: &str, status: &str) -> String {
    format!(
        "Queue {queue_name} under account {} delivers records in state {QUEUE_ACTIVE}, \
         instead in state {status}.",
        ::engine::emulator_account_id()
    )
}

/// How a create refuses a queue this emulator has no room for: the ceilings
/// ([`MAX_QUEUES_PER_REGION`], [`MAX_QUEUES_TOTAL`]) are on what the store holds once the
/// request is served, so the refusal states what it already holds — the same sentence shape
/// the channel ceiling states ([`channel_ceiling_exceeded_message`]).
pub fn queue_ceiling_exceeded_message(held_in_region: usize, held: usize) -> String {
    format!(
        "this region holds {held_in_region} queues and this emulator {held}, and another \
         would exceed the {MAX_QUEUES_PER_REGION} queues one region and the \
         {MAX_QUEUES_TOTAL} queues this emulator may hold; delete one before creating another"
    )
}

/// How a create refuses a `StreamConfigurationList` naming more than one data stream: "The
/// list must contain exactly one entry, because a queue delivers records from a single data
/// stream" (`CreateQueueInput$StreamConfigurationList`). Built FROM the request, so the count
/// at fault names it.
pub fn one_stream_per_queue_message(requested: usize) -> String {
    format!(
        "StreamConfigurationList names {requested} data streams; the list must contain \
         exactly one entry, because a queue delivers records from a single data stream"
    )
}

/// How a create refuses an `AT_TIMESTAMP` starting position carrying no instant: the instant
/// "is required in that case" (`QueueStartingPosition$Timestamp`). FIXED text — it carries no
/// request- or state-derived value beyond the position type the rule is about.
pub const STARTING_POSITION_TIMESTAMP_REQUIRED_MESSAGE: &str =
    "StartingPosition.Timestamp is required with the AT_TIMESTAMP starting position type";

/// How a create refuses an instant on a starting position that takes none: "specifying it for
/// any other type returns a ValidationException" (`QueueStartingPosition$Timestamp`). Built
/// FROM the request, so the type at fault names it — the same sentence shape the other reader
/// of a position states ([`super::shard_reading`]).
pub fn starting_position_timestamp_unsupported_message(position_type: &str) -> String {
    format!(
        "StartingPosition.Timestamp can only be used with the \
         {QUEUE_STARTING_POSITION_AT_TIMESTAMP} starting position type, not with \
         {position_type}"
    )
}

/// How a create or an update refuses built-in dead-letter handling that names no receive
/// count: "If you specify it with a TargetType of BUILT_IN, you must also specify
/// MaxReceiveCount" (`QueueAttributes$DeadLetterConfiguration`). FIXED text — it carries no
/// request- or state-derived value.
pub const MAX_RECEIVE_COUNT_REQUIRED_MESSAGE: &str =
    "DeadLetterConfiguration.MaxReceiveCount is required when TargetType is BUILT_IN";

/// How a delete of a STREAM refuses one that still carries queues: "You must delete every
/// queue on a data stream before you can delete the stream, or switch it from on-demand to
/// provisioned capacity mode" (`DeleteQueue`) — a queue on a stream is a dependency of that
/// stream, so the queues have to go first. Built FROM the stored record and this account, the
/// same way the channel dependency is worded ([`stream_has_channels_message`]).
pub fn stream_has_queues_message(stream_name: &str) -> String {
    format!(
        "Stream {stream_name} under account {} cannot be deleted because it is associated \
         with one or more queues. Delete all associated queues before deleting the stream.",
        ::engine::emulator_account_id()
    )
}

/// How a capacity-mode change refuses a stream that still carries queues, which is the other
/// half of the same documented rule: queues are "supported only on data streams in on-demand
/// capacity mode" (`CreateQueue`), so a stream carrying one cannot be switched to the
/// provisioned mode until they are deleted.
pub fn stream_has_queues_for_mode_change_message(stream_name: &str) -> String {
    format!(
        "Stream {stream_name} under account {} cannot be switched to {} capacity mode \
         because it is associated with one or more queues. Delete all associated queues \
         before switching the capacity mode of the stream.",
        ::engine::emulator_account_id(),
        capacity_mode_prose(STREAM_MODE_PROVISIONED),
    )
}

/// How a batch entry states that the receipt handle it carried names no record the queue is
/// tracking: a handle is issued per delivery, so one from a delivery that is over, one of
/// another queue and one the service never issued all name no record. Built FROM the request,
/// and it quotes no handle back: the entry it is reported in already carries the handle.
pub const RECEIPT_HANDLE_NOT_FOUND_MESSAGE: &str =
    "The receipt handle is not one this queue issued for a record it is tracking.";

/// The `ErrorCode` a batch entry carries when its receipt handle names no record: one of
/// "ResourceNotFoundException, InvalidArgumentException, ThrottlingException, or
/// InternalFailureException" (`AcknowledgeQueueRecordsErrorEntry$ErrorCode`), and the one of
/// them that names a resource that is not there.
pub const ENTRY_ERROR_CODE_NOT_FOUND: &str = "ResourceNotFoundException";


/// The stored instant the timestamp `at` a REQUEST carried names, or `None` when that
/// timestamp names no instant this emulator can hold.
///
/// Two rejections, both of a value a caller controls: one too far from the epoch to be a
/// millisecond count at all ([`requested_millis`]), and one so far from it that the instant it
/// names cannot be represented on this host — the add is CHECKED, because a request-controlled
/// addend reaching the panicking operator would unwind the handler.
///
/// ONE rule for every queue member that carries an epoch date, so the create and any later
/// reader read one the same way.
pub fn requested_instant(at: ::aws_smithy_types::DateTime) -> Option<SystemTime> {
    let millis = u64::try_from(requested_millis(at)?).ok()?;
    UNIX_EPOCH.checked_add(Duration::from_millis(millis))
}

/// The ARN a queue created in `region` at `created_at` under `name` is addressed by, inside the
/// `QueueARN` pattern (`^arn:aws.*:kinesis:.*:\d{12}:queue/\S+$`).
///
/// "The resource identifier in the ARN is generated when the queue is created and is not
/// derived from QueueName" (`QueueDescription$QueueARN`), so the resource segment is an
/// identifier DERIVED from what identifies the queue — its region, its name and the instant it
/// was created at — through a digest crate, the same way a channel's id is
/// ([`channel_id`]): two queues differ, a name re-created after a delete differs from the
/// queue it replaces, and the value a read reports never changes. Nothing here authenticates
/// or protects anything; the digest is only how the three parts are folded into one short id.
pub fn queue_arn(region: &str, name: &str, created_at: SystemTime) -> String {
    let seed = format!("queue/{region}/{name}/{}", epoch_millis(created_at));
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(seed.as_bytes());
    ::engine::emulator_arn(
        SERVICE_ARN_NAMESPACE,
        region,
        &format!("queue/{}", ::hex::encode(&digest[..RECORD_ID_DIGEST_BYTES])),
    )
}

/// The `RecordId` of the stream record `sequence_number` names on `shard_id` of the stream
/// `stream_arn` names: "This value stays the same across every delivery of the record, and it
/// is the same in every queue on the stream that delivers the record"
/// (`QueueRecord$RecordId`), so it is derived from the record's place in the STREAM and from
/// nothing of the queue. Hex-encoded, which is inside the member's `^[a-z0-9]+$` pattern and
/// its `@length` of 1..64.
pub fn record_id(stream_arn: &str, shard_id: &str, sequence_number: &str) -> String {
    let seed = format!("{stream_arn}/{shard_id}/{sequence_number}");
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(seed.as_bytes());
    ::hex::encode(&digest[..RECORD_ID_DIGEST_BYTES])
}

/// The receipt handle the `deliveries`-th delivery of the record `record_id` names out of the
/// queue `queue_arn` names is issued under: "The identifier you use to acknowledge this
/// record, change its visibility, or restore it after it becomes a dead-letter record. A new
/// receipt handle is issued each time the record is received"
/// (`QueueRecord$ReceiptHandle`), inside its modelled `@length` of 1..1024.
///
/// Opaque to a client, and derived from the queue, the record and which delivery it is, so no
/// two deliveries of one record share a handle and a handle of one queue names nothing in
/// another.
pub fn receipt_handle(queue_arn: &str, record_id: &str, deliveries: u64) -> String {
    let seed = format!("{queue_arn}/{record_id}/{deliveries}");
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(seed.as_bytes());
    ::hex::encode(digest)
}
