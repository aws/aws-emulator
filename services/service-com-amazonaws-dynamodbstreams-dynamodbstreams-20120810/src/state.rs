// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dynamodbstreams-dynamodbstreams-20120810 service.
//!
//! DynamoDB Streams models no create or delete API of its own: a stream comes into
//! existence when DynamoDB's `CreateTable`/`UpdateTable` is given a
//! `StreamSpecification`, and records are published by `PutItem`/`UpdateItem`/
//! `DeleteItem`. All four modelled operations are therefore pure consumers, so this
//! module owns both the consumer-visible store and the publishing seam the DynamoDB
//! service calls into: `register_stream`, `disable_stream`, `append_record` and
//! `roll_shard` on the state struct below.
//!
//! Positions inside a shard are ABSOLUTE record ordinals (`trimmed + offset`), not
//! indices into the retained `Vec`, so trimming the oldest records past the
//! retention limit never silently moves a shard iterator that a client is still
//! holding.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{AttributeValue, KeySchemaElement};

/// A DynamoDB item image or key map, as the modelled `AttributeMap` shape.
pub type AttributeMap = HashMap<String, AttributeValue>;

/// How long a shard iterator stays usable.
///
/// From the model's `GetShardIterator` documentation: "A shard iterator expires 15
/// minutes after it is returned to the requester."
pub const SHARD_ITERATOR_TTL: Duration = Duration::from_secs(15 * 60);

/// How long a stream record is retained before it is trimmed.
///
/// From the model's `TrimmedDataAccessException` documentation: "In DynamoDB
/// Streams, there is a 24 hour limit on data retention."
pub const RECORD_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// How long after registration a stream reports `ENABLED` instead of `ENABLING`.
///
/// DynamoDB enables a stream asynchronously, which is why the model documents
/// `StreamStatus` `ENABLING` ("Streams is currently being enabled on the DynamoDB
/// table") beside `ENABLED`. The real wait is not documented, so the span is this
/// emulator's: it is the ONE span every clock-owed transition of this project takes,
/// three seconds, which is long enough that a caller walking a whole sequence of
/// calls after the one that registered the stream still observes the transient
/// status rather than having it settle between two of its own requests. It is also
/// never shorter than the table's own transition
/// (`service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION`),
/// which the seam that registers a stream asserts at compile time.
pub const STREAM_ENABLE_DELAY: Duration = Duration::from_secs(3);

/// How long after a disable request a stream reports `DISABLED` instead of
/// `DISABLING`.
///
/// The mirror of [`STREAM_ENABLE_DELAY`], over the same span: the model documents
/// `StreamStatus` `DISABLING` ("Streams is currently being disabled on the DynamoDB
/// table") beside `DISABLED`, so a caller that describes the stream straight after the
/// call — or anywhere inside the span — still observes the transient status.
pub const STREAM_DISABLE_DELAY: Duration = STREAM_ENABLE_DELAY;

/// The most records one `GetRecords` call may return.
///
/// From the model's `GetRecords` documentation: "GetRecords can retrieve a maximum
/// of 1 MB of data or 1000 stream records, whichever comes first."
pub const MAX_GET_RECORDS_LIMIT: i32 = 1000;

/// The most data one `GetRecords` call may return, the other half of the same
/// documented sentence ("a maximum of 1 MB of data or 1000 stream records,
/// whichever comes first"). Measured over the records' own `SizeBytes`.
pub const MAX_GET_RECORDS_BYTES: i64 = 1024 * 1024;

/// How long a stream stays registered after it is DISABLED, before the clock
/// reclaims it.
///
/// From the model's `DeleteTable` documentation: "If you have DynamoDB Streams
/// enabled on the table, then the corresponding stream on that table goes into
/// the DISABLED state, and the stream is automatically deleted after 24 hours."
/// That deletion is what bounds the registry over time: a stream a caller turned
/// off is drainable for the documented window and then gone.
pub const DISABLED_STREAM_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// How many streams this registry holds for ONE table.
///
/// A table has at most one stream ON; the rest are disabled predecessors, which a
/// caller produces by turning a stream off and on again and which
/// [`DISABLED_STREAM_RETENTION`] reclaims. AWS publishes no quota for that count,
/// so this is a CHOSEN bound, and it is what keeps the registry's size a function
/// of the tables that exist rather than of how often a caller toggles one: past it
/// a registration is refused and the control plane answers its own
/// `LimitExceededException`.
///
/// This bound is the emulator's, NOT a modelled AWS quota: real DynamoDB would
/// accept a 33rd enable within the retention window. It is the alternative to
/// keeping every disabled stream a caller can ask for, and it is deliberately
/// generous enough that no ordinary enable/disable sequence meets it.
///
/// It is counted over the streams whose ARN carries this table's NAME, so the
/// streams of a table that has been deleted still count against a new table
/// created with the same name until [`DISABLED_STREAM_RETENTION`] reclaims them.
/// That is the same resource the name owns, which is why the count is read off the
/// name rather than off a table identity this service does not hold.
pub const MAX_STREAMS_PER_TABLE: usize = 32;

/// How many streams this registry holds in total — the second half of the same
/// bound, so the registry is capped even if the control plane's own table quotas
/// change. Also a CHOSEN figure.
pub const MAX_STREAMS: usize = 10_000;

/// How many LIVE shard iterators this service holds.
///
/// An iterator is reclaimed by its own 15 minute lifetime
/// ([`SHARD_ITERATOR_TTL`]), but that alone bounds the table only by how fast a
/// client asks for iterators, so the count has a ceiling of its own: a mint past
/// it RETIRES the oldest live iterator instead of growing the store. Retiring
/// rather than dropping is what keeps the answer honest — a client that comes back
/// with an evicted token is told its iterator expired, which is what happened to
/// it. Also a CHOSEN figure, and generous: no consumer loop needs this many
/// iterators alive at once.
pub const MAX_LIVE_ITERATORS: usize = 10_000;

/// How many RETIRED shard-iterator tokens this service remembers.
///
/// A retired token is the only record that this service ever minted it, which is
/// what tells an iterator past its lifetime (`ExpiredIteratorException`) from a
/// token that is not one of this service's iterators at all (a request this
/// service refuses as invalid). The set is capped so a client cannot grow the
/// store without limit by asking for iterators, and the OLDEST retirement is
/// dropped first: a token retired that long ago is no longer attributable, so it
/// reads as not-an-iterator again.
pub const MAX_RETIRED_ITERATORS: usize = 10_000;

/// The most entries one page of `ListStreams` or `DescribeStream` may return.
///
/// From the model's `ListStreamsInput$Limit` ("The maximum number of streams to
/// return. The upper limit is 100.") and `DescribeStreamInput$Limit` ("The maximum
/// number of shard objects to return. The upper limit is 100.").
pub const MAX_PAGE_LIMIT: i32 = 100;

/// `eventSource` for every DynamoDB Streams record: the model's
/// `Record$eventSource` says "For DynamoDB Streams, this is `aws:dynamodb`".
pub const EVENT_SOURCE: &str = "aws:dynamodb";

/// How deeply an item image handed to [`ComAmazonawsDynamodbstreamsDynamodbstreams20120810State::append_record`]
/// may nest.
///
/// DynamoDB's published quota: "Attribute nesting is limited to 32 levels deep."
/// (Service, Account, and Table Quotas in Amazon DynamoDB — Attributes.) An image
/// the publisher itself would have refused is refused here too, which also bounds
/// every later walk over a stored image (sizing, serialization) by a constant
/// instead of by whatever a client sent.
pub const MAX_ATTRIBUTE_NESTING_DEPTH: usize = 32;

/// The most record bytes ONE stream retains, past which the oldest record is trimmed
/// ([`StoredStream::trim_to_retained_budget`]).
///
/// The records of a stream grow with the item writes a caller makes to its table —
/// one record per modification, each holding the images the stream captures — and the
/// 24 hour retention alone would let a client that rewrites one item in a loop grow
/// this store without end inside that window. AWS publishes no byte figure for a
/// stream's retained data, so this ceiling is this emulator's own, chosen to hold the
/// records of a busy table while keeping what one process can accumulate bounded; a
/// record past it is TRIMMED, which is the same thing the documented 24 hours does to
/// records that age out, and is reported the same way.
pub const MAX_RETAINED_STREAM_BYTES: i64 = 64 * 1024 * 1024;

/// The stream-record format version this emulator emits (`Record$eventVersion`).
pub const EVENT_VERSION: &str = "1.1";

/// `SequenceNumber` is a decimal string of 21..40 characters ("String contains
/// numeric characters only"), so the monotonic counter is offset into the 21-digit
/// range: every generated sequence number is exactly 21 digits.
const SEQUENCE_NUMBER_BASE: u128 = 100_000_000_000_000_000_000;

/// The format version a shard-iterator token carries between its stream ARN and its
/// opaque blob, as a DynamoDB Streams iterator does. It is the token FORMAT's
/// version, not a position: a token of an older format could be recognised and
/// refused rather than misread.
const ITERATOR_TOKEN_VERSION: u32 = 1;

/// The bytes an iterator's third segment OPENS with, ahead of its payload length and
/// its payload.
///
/// The model says nothing about the token's internal layout, so this comes from the
/// tokens DynamoDB Streams itself hands out: their third segment is a base64
/// envelope that always begins with the fixed preamble `AAAAAAAAAAH`, which is
/// exactly what base64 renders these nine bytes as. That preamble and the segment's
/// length are the part of the token a client or a proxy can observe and check —
/// length-budgeting toward the modelled 2048 bound, or sanity-checking the segment
/// before forwarding it — so they are reproduced here; the payload BYTES after it
/// are opaque and are nobody's to check.
const ITERATOR_ENVELOPE_PREAMBLE: [u8; 9] = [0, 0, 0, 0, 0, 0, 0, 1, 0xC0];

/// How many payload bytes an iterator's envelope carries after its preamble and the
/// `u16` length of this payload.
///
/// From the same source as [`ITERATOR_ENVELOPE_PREAMBLE`]: DynamoDB Streams'
/// envelope renders as roughly 950 base64 characters, so the payload is sized to
/// put the segment there — `9 + 2 + 700` is 711 bytes, a multiple of three, which
/// base64 renders as 948 characters with no padding. With the longest stream ARN a
/// table name can produce the whole token still leaves hundreds of characters of
/// room under the modelled `ShardIterator` maximum of 2048.
const ITERATOR_PAYLOAD_BYTES: usize = 700;

/// The payload length the envelope declares, as the two big-endian bytes of its `u16`
/// length field.
///
/// The field is two bytes wide, so the payload has to fit one: that is checked HERE, where
/// the size is a constant, rather than papered over at mint time with a length the envelope
/// would then disagree with.
const _: () = assert!(ITERATOR_PAYLOAD_BYTES <= u16::MAX as usize);
const ITERATOR_PAYLOAD_LENGTH_FIELD: [u8; 2] = (ITERATOR_PAYLOAD_BYTES as u16).to_be_bytes();

/// The format of the records a stream captures (the modelled `StreamViewType`
/// enum; wire values from `STREAM_VIEW_TYPE_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamViewType {
    KeysOnly,
    NewImage,
    OldImage,
    NewAndOldImages,
}

impl StreamViewType {
    /// The wire value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KeysOnly => "KEYS_ONLY",
            Self::NewImage => "NEW_IMAGE",
            Self::OldImage => "OLD_IMAGE",
            Self::NewAndOldImages => "NEW_AND_OLD_IMAGES",
        }
    }

    /// Parse a wire value; `None` for anything the enum does not declare.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "KEYS_ONLY" => Some(Self::KeysOnly),
            "NEW_IMAGE" => Some(Self::NewImage),
            "OLD_IMAGE" => Some(Self::OldImage),
            "NEW_AND_OLD_IMAGES" => Some(Self::NewAndOldImages),
            _ => None,
        }
    }

    /// Whether this view type captures the item as it looked after the change.
    #[must_use]
    pub fn captures_new_image(self) -> bool {
        matches!(self, Self::NewImage | Self::NewAndOldImages)
    }

    /// Whether this view type captures the item as it looked before the change.
    #[must_use]
    pub fn captures_old_image(self) -> bool {
        matches!(self, Self::OldImage | Self::NewAndOldImages)
    }
}

/// The kind of data modification a record describes (the modelled `OperationType`
/// enum; wire values from `OPERATION_TYPE_VALUES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    Insert,
    Modify,
    Remove,
}

impl OperationType {
    /// The wire value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "INSERT",
            Self::Modify => "MODIFY",
            Self::Remove => "REMOVE",
        }
    }

    /// Parse a wire value; `None` for anything the enum does not declare.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "INSERT" => Some(Self::Insert),
            "MODIFY" => Some(Self::Modify),
            "REMOVE" => Some(Self::Remove),
            _ => None,
        }
    }
}

/// A stream's lifecycle status (the modelled `StreamStatus` enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStatus {
    Enabling,
    Enabled,
    Disabling,
    Disabled,
}

impl StreamStatus {
    /// The wire value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enabling => "ENABLING",
            Self::Enabled => "ENABLED",
            Self::Disabling => "DISABLING",
            Self::Disabled => "DISABLED",
        }
    }
}

/// What the publisher (DynamoDB `CreateTable`/`UpdateTable` with a
/// `StreamSpecification`) tells this service about a new stream.
#[derive(Debug, Clone)]
pub struct StreamRegistration {
    pub table_name: String,
    pub stream_view_type: StreamViewType,
    pub key_schema: Vec<KeySchemaElement>,
    pub region: String,
}

/// One item-level change the publisher is handing to the stream. Images outside
/// the stream's `StreamViewType` are dropped on append, so a consumer can never
/// see more than the stream captures.
#[derive(Debug, Clone)]
pub struct RecordAppend {
    pub event_name: OperationType,
    pub keys: AttributeMap,
    pub new_image: Option<AttributeMap>,
    pub old_image: Option<AttributeMap>,
}

/// Why [`ComAmazonawsDynamodbstreamsDynamodbstreams20120810State::append_record`]
/// refused a publication, so the publisher can word its own failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendRejected {
    /// No stream with that ARN is registered.
    UnknownStream,
    /// Every shard of the stream is closed, so nothing can be appended.
    NoOpenShard,
    /// An image nests deeper than [`MAX_ATTRIBUTE_NESTING_DEPTH`].
    NestingTooDeep { depth: usize, limit: usize },
}

impl std::fmt::Display for AppendRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownStream => write!(f, "no stream with that ARN is registered"),
            Self::NoOpenShard => write!(f, "the stream has no open shard to append to"),
            Self::NestingTooDeep { depth, limit } => write!(
                f,
                "an item image nests {depth} levels deep, past the {limit} level limit"
            ),
        }
    }
}

/// Why [`ComAmazonawsDynamodbstreamsDynamodbstreams20120810State::register_stream`]
/// refused to register a stream, so the publisher can word its own failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationRejected {
    /// The table already holds [`MAX_STREAMS_PER_TABLE`] streams.
    TooManyStreamsForTable { table_name: String, limit: usize },
    /// The registry already holds [`MAX_STREAMS`] streams.
    TooManyStreams { limit: usize },
    /// No free stream label within the bounded probe — see `register_stream`.
    NoFreeLabel { table_name: String, probes: usize },
}

impl std::fmt::Display for RegistrationRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyStreamsForTable { table_name, limit } => write!(
                f,
                "table {table_name} already holds the maximum of {limit} streams"
            ),
            Self::TooManyStreams { limit } => write!(
                f,
                "this account already holds the maximum of {limit} streams"
            ),
            Self::NoFreeLabel {
                table_name,
                probes,
            } => write!(
                f,
                "no free stream label for table {table_name} in {probes} attempts"
            ),
        }
    }
}

/// A stream record as it is persisted.
#[derive(Debug, Clone)]
pub struct StoredRecord {
    pub event_id: String,
    pub event_name: OperationType,
    pub event_version: String,
    pub event_source: String,
    pub aws_region: String,
    pub sequence_number: String,
    /// When the change was published, floored to a whole second by
    /// [`floor_to_second`] — the granularity the member it feeds is reported at.
    pub approximate_creation_date_time: SystemTime,
    /// The instant of the WRITE itself, at the resolution the clock gave it.
    ///
    /// The member above is an APPROXIMATION of this instant, "rounded down to the
    /// closest second", so it names a moment up to a second BEFORE the write: a span
    /// measured from it — the age of a record — is that much too long, which is what
    /// this field is for. It is never reported: what a consumer reads is the member,
    /// at the resolution the model documents.
    pub written_at: SystemTime,
    pub keys: AttributeMap,
    pub new_image: Option<AttributeMap>,
    pub old_image: Option<AttributeMap>,
    pub size_bytes: i64,
}

/// A shard: an append-ordered run of records, open until it is rolled over.
#[derive(Debug, Clone)]
pub struct StoredShard {
    pub shard_id: String,
    pub parent_shard_id: Option<String>,
    pub starting_sequence_number: String,
    /// `None` while the shard is open (still writable), which is why
    /// `SequenceNumberRange.EndingSequenceNumber` is absent for an open shard.
    pub ending_sequence_number: Option<String>,
    /// The records still retained, oldest first.
    pub records: VecDeque<StoredRecord>,
    /// How many records this shard has already trimmed past the retention limit.
    pub trimmed: u64,
}

impl StoredShard {
    /// Whether the shard is still writable.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.ending_sequence_number.is_none()
    }

    /// Total records ever appended, trimmed ones included: the absolute position
    /// just past the newest record, which is where `LATEST` starts.
    #[must_use]
    pub fn appended(&self) -> u64 {
        self.trimmed + self.records.len() as u64
    }

    /// The absolute position of the oldest retained record, which is where
    /// `TRIM_HORIZON` starts.
    #[must_use]
    pub fn trim_horizon(&self) -> u64 {
        self.trimmed
    }

    /// Up to `limit` records starting at absolute position `position`. Empty when
    /// the position is already past the newest record.
    #[must_use]
    pub fn records_from(&self, position: u64, limit: usize) -> Vec<&StoredRecord> {
        let Some(offset) = position.checked_sub(self.trimmed) else {
            return Vec::new();
        };
        let Ok(offset) = usize::try_from(offset) else {
            return Vec::new();
        };
        self.records.iter().skip(offset).take(limit).collect()
    }
}

/// A stream and the shards that comprise it.
#[derive(Debug, Clone)]
pub struct StoredStream {
    pub stream_arn: String,
    pub table_name: String,
    pub stream_label: String,
    pub stream_view_type: StreamViewType,
    pub creation_request_date_time: SystemTime,
    /// When the asynchronous enable this registration started becomes due.
    pub enabled_at: SystemTime,
    /// When the asynchronous disable a `StreamSpecification` that turns the
    /// stream OFF started becomes due, or `None` while the stream is on. The
    /// stream stays REGISTERED once disabled — AWS deletes it only after 24
    /// hours — so a consumer still discovers it and reads its status.
    pub disabled_at: Option<SystemTime>,
    pub aws_region: String,
    pub key_schema: Vec<KeySchemaElement>,
    /// Shards in creation order: a child shard always follows its parent.
    pub shards: Vec<StoredShard>,
    /// The summed `SizeBytes` of every record this stream still retains.
    ///
    /// Tracked rather than summed per call, because it is read on EVERY append: it is
    /// what [`MAX_RETAINED_STREAM_BYTES`] is measured against, so the retention a
    /// stream enforces costs a comparison instead of a walk over what it holds.
    pub retained_bytes: i64,
}

impl StoredStream {
    /// The status the clock owes at `now`: a registration starts the stream
    /// `ENABLING` and it becomes `ENABLED` once the enable is due; a disable
    /// starts it `DISABLING` and it becomes `DISABLED` once that is due.
    #[must_use]
    pub fn status_at(&self, now: SystemTime) -> StreamStatus {
        match self.disabled_at {
            Some(due) if now >= due => StreamStatus::Disabled,
            Some(_) => StreamStatus::Disabling,
            None if now < self.enabled_at => StreamStatus::Enabling,
            None => StreamStatus::Enabled,
        }
    }

    /// The shard with this id, if the stream has one.
    #[must_use]
    pub fn shard(&self, shard_id: &str) -> Option<&StoredShard> {
        self.shards.iter().find(|shard| shard.shard_id == shard_id)
    }

    /// Trim the OLDEST records this stream retains until it is back inside
    /// [`MAX_RETAINED_STREAM_BYTES`].
    ///
    /// The same mechanism the 24 hour retention uses, for the same reason: records
    /// accumulate with the writes a caller makes to the table, and a client that
    /// overwrites one item in a loop would otherwise grow this store without end
    /// inside that window. A trimmed record is counted on its shard, so the shard's
    /// `TRIM_HORIZON` moves with it and a consumer reading from a position that is
    /// gone is answered with the modelled `TrimmedDataAccessException` rather than
    /// with a gap.
    fn trim_to_retained_budget(&mut self) {
        while self.retained_bytes > MAX_RETAINED_STREAM_BYTES {
            let Some(shard) = self
                .shards
                .iter_mut()
                .find(|shard| !shard.records.is_empty())
            else {
                // Nothing is retained, so the figure is stale rather than the store
                // being over budget: there is nothing left to trim.
                self.retained_bytes = 0;
                return;
            };
            let Some(trimmed) = shard.records.pop_front() else {
                return;
            };
            shard.trimmed += 1;
            self.retained_bytes = self.retained_bytes.saturating_sub(trimmed.size_bytes);
        }
    }

    /// The shards a CONSUMER can read at `now`.
    ///
    /// A stream is `ENABLING` while "Streams is currently being enabled on the
    /// DynamoDB table" (`StreamStatus`), so the enable the clock owes has not
    /// finished and the stream has nothing to read yet: it exposes NO shards until
    /// it is enabled. That is what makes the status a consumer gates on agree with
    /// what the stream serves — a stream that hands out shards and iterators is
    /// `ENABLED`, never `ENABLING` — and it is why the shards of a table's stream
    /// appear only once the table behind them is serving.
    ///
    /// A `DISABLING`/`DISABLED` stream keeps its shards: AWS deletes it only after
    /// the documented 24 hours, so a consumer can still drain what it captured.
    #[must_use]
    pub fn shards_at(&self, now: SystemTime) -> &[StoredShard] {
        if self.status_at(now) == StreamStatus::Enabling {
            &[]
        } else {
            &self.shards
        }
    }

    /// The shard with this id as a consumer can read it at `now` — the lookup
    /// [`Self::shards_at`] scopes, so a shard of a stream whose enable is not due
    /// yet is not addressable either.
    #[must_use]
    pub fn shard_at(&self, shard_id: &str, now: SystemTime) -> Option<&StoredShard> {
        self.shards_at(now)
            .iter()
            .find(|shard| shard.shard_id == shard_id)
    }
}

/// What a `ShardIterator` token a client presented resolves to.
///
/// The token's opaque segment is an envelope over a digest, so it carries no
/// position and cannot be decoded: the only thing that makes a token an iterator is
/// that this service minted it and still remembers doing so. That is what
/// separates the two refusals `GetRecords` can answer — an iterator whose lifetime
/// has passed, and a value that is not an iterator.
#[derive(Debug, Clone, Copy)]
pub enum IteratorLookup<'a> {
    /// A live iterator, standing at the position it carries.
    Live(&'a IteratorState),
    /// A token this service minted whose 15 minute lifetime has since passed.
    Retired,
    /// A token this service has no record of minting, so it names no position.
    Unknown,
}

/// Where a shard iterator token points, and when it was handed out.
#[derive(Debug, Clone)]
pub struct IteratorState {
    pub stream_arn: String,
    pub shard_id: String,
    /// The ABSOLUTE position of the next record to deliver.
    pub next_position: u64,
    pub issued_at: SystemTime,
}

/// Per-service state for com-amazonaws-dynamodbstreams-dynamodbstreams-20120810. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDynamodbstreamsDynamodbstreams20120810State {
    /// Registered streams, keyed and ordered by stream ARN so `ListStreams` pages
    /// deterministically.
    streams: BTreeMap<String, StoredStream>,
    /// Live shard iterators, keyed by [`iterator_key`] of the token handed to the
    /// client — a fixed-size derivation of the whole token, so what these capped
    /// tables hold is a function of their caps rather than of how long a token is.
    iterators: HashMap<String, IteratorState>,
    /// The live iterator keys in MINT order (oldest first), so the
    /// [`MAX_LIVE_ITERATORS`] cap can retire the oldest. It may also hold keys
    /// the clock has already retired; those are skipped when they come up, and its
    /// own length is what the cap is enforced on, so `iterators` can never be
    /// larger than the cap.
    live_iterator_keys: VecDeque<String>,
    /// The keys of tokens this service minted whose lifetime has passed, in
    /// retirement order (oldest first) so the cap drops the oldest. Paired with
    /// `retired_iterator_keys`, which answers membership in one hash.
    retired_iterator_order: VecDeque<String>,
    /// Membership index over `retired_iterator_order`; the two always hold the same
    /// keys, because [`ComAmazonawsDynamodbstreamsDynamodbstreams20120810State::retire_iterator`]
    /// is the only thing that writes either.
    retired_iterator_keys: HashSet<String>,
    /// Backs sequence-number generation; monotonic for the life of the emulator.
    next_sequence_number: u64,
    /// Backs shard-id generation, so two shards created in the same millisecond
    /// still get distinct ids.
    next_shard_ordinal: u64,
    /// Backs iterator-token generation, so two iterators on the same position are
    /// still distinct tokens.
    next_iterator_ordinal: u64,
}

impl ComAmazonawsDynamodbstreamsDynamodbstreams20120810State {
    /// Every registered stream, in stream-ARN order.
    #[must_use]
    pub fn streams(&self) -> &BTreeMap<String, StoredStream> {
        &self.streams
    }

    /// The stream with this ARN, if one is registered.
    #[must_use]
    pub fn stream(&self, stream_arn: &str) -> Option<&StoredStream> {
        self.streams.get(stream_arn)
    }

    /// WHEN the write that produced the record `sequence_number` names in the stream
    /// `stream_arn` was made, at the resolution the clock gave it — `None` for a stream
    /// this service does not hold and for a record it no longer retains.
    ///
    /// What a consumer READS for that record is
    /// `StreamRecord.ApproximateCreationDateTime`, "rounded down to the closest second",
    /// which names a moment up to a second before the write. A span measured from that
    /// member is that much too long, so anything in this emulator that meters how long a
    /// record has been in the shard asks here instead
    /// ([`StoredRecord::written_at`]).
    ///
    /// A shard's retained records are in APPEND order, which is ascending sequence number
    /// ([`compare_sequence_numbers`]), so the record is found by halving each shard's run
    /// rather than by walking it: this is read once per batch a consumer meters.
    #[must_use]
    pub fn record_written_at(
        &self,
        stream_arn: &str,
        sequence_number: &str,
    ) -> Option<SystemTime> {
        for shard in &self.streams.get(stream_arn)?.shards {
            let found = shard.records.binary_search_by(|record| {
                compare_sequence_numbers(&record.sequence_number, sequence_number)
            });
            if let Ok(at) = found {
                return shard.records.get(at).map(|record| record.written_at);
            }
        }
        None
    }

    /// How many iterators are LIVE — the figure [`MAX_LIVE_ITERATORS`] caps.
    #[must_use]
    pub fn live_iterator_count(&self) -> usize {
        self.iterators.len()
    }

    /// What a client-presented `ShardIterator` token resolves to: a live
    /// iterator, one this service minted and has since retired, or a value it
    /// never minted at all.
    #[must_use]
    pub fn resolve_iterator(&self, token: &str) -> IteratorLookup<'_> {
        let key = iterator_key(token);
        if let Some(iterator) = self.iterators.get(&key) {
            return IteratorLookup::Live(iterator);
        }
        if self.retired_iterator_keys.contains(&key) {
            return IteratorLookup::Retired;
        }
        IteratorLookup::Unknown
    }

    /// Materialise everything the CLOCK already owes at `now`, whether or not
    /// anybody called: a stream disabled longer than
    /// [`DISABLED_STREAM_RETENTION`] is deleted, records past the 24 hour
    /// retention limit are trimmed, and shard iterators past their 15 minute
    /// lifetime are retired.
    pub fn materialize_due_expiries(&mut self, now: SystemTime) {
        // "The stream is automatically deleted after 24 hours" (the `DeleteTable`
        // doc): a stream whose disable fell due that long ago is gone, which is what
        // reclaims the registry rather than letting a disabled stream sit in it for
        // the life of the process.
        self.streams.retain(|_, stream| {
            stream
                .disabled_at
                .is_none_or(|due| !older_than(due, now, DISABLED_STREAM_RETENTION))
        });
        for stream in self.streams.values_mut() {
            let mut reclaimed: i64 = 0;
            for shard in &mut stream.shards {
                while shard
                    .records
                    .front()
                    .is_some_and(|record| older_than(record.approximate_creation_date_time, now, RECORD_RETENTION))
                {
                    reclaimed = reclaimed
                        .saturating_add(shard.records.pop_front().map_or(0, |record| record.size_bytes));
                    shard.trimmed += 1;
                }
            }
            stream.retained_bytes = stream.retained_bytes.saturating_sub(reclaimed);
        }
        // An iterator past its lifetime stops standing at a position, but the fact
        // that this service minted it is kept: that is what makes it an EXPIRED
        // iterator rather than a value that was never one. An iterator of a stream
        // the clock has deleted is left alone and reclaimed by this same lifetime,
        // so `GetRecords` can still answer the modelled `ResourceNotFoundException`
        // for the stream that is gone rather than reporting the iterator as the
        // problem.
        let expired: Vec<String> = self
            .iterators
            .iter()
            .filter(|(_, iterator)| older_than(iterator.issued_at, now, SHARD_ITERATOR_TTL))
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            self.iterators.remove(&key);
            self.retire_iterator(key);
        }
    }

    /// Register a stream for a table — the publishing seam DynamoDB's
    /// `CreateTable`/`UpdateTable` calls when a `StreamSpecification` enables one.
    ///
    /// Returns the stream ARN. Idempotent while the stream is ON: registering the
    /// same table at the same instant yields the same ARN and leaves the existing
    /// stream untouched, so a repeated registration does not fork a second stream.
    ///
    /// A stream's ARN carries its label, and a label identifies at most ONE stream
    /// of a table — "the combination of ... Amazon Web Services customer ID, table
    /// name, StreamLabel is guaranteed to be unique"
    /// (`TableDescription$LatestStreamLabel`) — so a registration landing on the
    /// label of a stream that has since been DISABLED takes the next free
    /// millisecond instead of resurrecting it: that stream keeps the records it
    /// captured until it is deleted, and a re-enabled stream is a new one.
    /// REFUSED past [`MAX_STREAMS_PER_TABLE`] or [`MAX_STREAMS`], which is what
    /// keeps the registry bounded by the resources that exist rather than by how
    /// often a caller turns a stream off and on again; the control plane answers
    /// the refusal with its own `LimitExceededException`. The work this does is
    /// bounded too: one prefix range over the table's own streams, then at most
    /// `MAX_STREAMS_PER_TABLE` label probes, which is enough because a table under
    /// the cap can occupy at most that many labels (see the loop below).
    pub fn register_stream(
        &mut self,
        registration: &StreamRegistration,
        at: SystemTime,
    ) -> Result<String, RegistrationRejected> {
        let first = stream_arn_at(registration, at);
        // The same table's stream, registered at the same instant and still on:
        // nothing to register, and nothing to count against a quota either.
        if self
            .streams
            .get(&first)
            .is_some_and(|existing| existing.disabled_at.is_none())
        {
            return Ok(first);
        }

        // How many streams this table already holds. A stream's ARN starts with its
        // table's own `table/<name>/stream/` prefix, so this is a RANGE over that
        // prefix in the ARN-ordered map rather than a scan of the registry.
        let prefix = stream_arn_prefix(&registration.region, &registration.table_name);
        let held = self
            .streams
            .range::<str, _>((
                std::ops::Bound::Included(prefix.as_str()),
                std::ops::Bound::Unbounded,
            ))
            .take_while(|(stream_arn, _)| stream_arn.starts_with(&prefix))
            .count();
        if held >= MAX_STREAMS_PER_TABLE {
            return Err(RegistrationRejected::TooManyStreamsForTable {
                table_name: registration.table_name.clone(),
                limit: MAX_STREAMS_PER_TABLE,
            });
        }
        if self.streams.len() >= MAX_STREAMS {
            return Err(RegistrationRejected::TooManyStreams {
                limit: MAX_STREAMS,
            });
        }

        // The table holds `held` streams, so at most `held` of its labels are taken
        // and a free one is reached within `held + 1` passes: a BOUNDED probe rather
        // than a search over whatever state a caller has built.
        let mut at = at;
        for _ in 0..=held {
            let stream_arn = stream_arn_at(registration, at);
            if self.streams.contains_key(&stream_arn) {
                at += Duration::from_millis(1);
                continue;
            }
            let shard = self.new_shard(&stream_arn, None, at);
            self.streams.insert(
                stream_arn.clone(),
                StoredStream {
                    stream_arn: stream_arn.clone(),
                    table_name: registration.table_name.clone(),
                    stream_label: iso8601_label(at),
                    stream_view_type: registration.stream_view_type,
                    creation_request_date_time: at,
                    enabled_at: at + STREAM_ENABLE_DELAY,
                    disabled_at: None,
                    aws_region: registration.region.clone(),
                    key_schema: registration.key_schema.clone(),
                    shards: vec![shard],
                    retained_bytes: 0,
                },
            );
            return Ok(stream_arn);
        }
        // Unreachable while the count above is the truth about this table's streams;
        // if it ever is not, this is a refusal rather than an unbounded search.
        Err(RegistrationRejected::NoFreeLabel {
            table_name: registration.table_name.clone(),
            probes: held + 1,
        })
    }

    /// Turn a registered stream OFF — the other half of the publishing seam, which
    /// DynamoDB's `UpdateTable` calls when a `StreamSpecification` disables the
    /// stream.
    ///
    /// The stream reports `DISABLING` until [`STREAM_DISABLE_DELAY`] has elapsed and
    /// `DISABLED` after that, and its open shard is CLOSED, so nothing more can be
    /// published to it while the records it already carries stay readable. It is not
    /// removed: AWS deletes a disabled stream only after 24 hours, so a consumer can
    /// still discover it and drain it. Idempotent, and `false` when no stream with
    /// that ARN is registered.
    pub fn disable_stream(&mut self, stream_arn: &str, at: SystemTime) -> bool {
        let Some(stream) = self.streams.get_mut(stream_arn) else {
            return false;
        };
        if stream.disabled_at.is_none() {
            stream.disabled_at = Some(at + STREAM_DISABLE_DELAY);
        }
        if let Some(open) = stream.shards.iter_mut().rfind(|shard| shard.is_open()) {
            // A shard that never took a record still needs an ending sequence number
            // of 21..40 digits, so it closes on its own starting number — the same
            // rule `roll_shard` closes a shard by.
            open.ending_sequence_number = Some(open.records.back().map_or_else(
                || open.starting_sequence_number.clone(),
                |record| record.sequence_number.clone(),
            ));
        }
        true
    }

    /// Publish one item-level change onto the stream's open shard — the seam
    /// DynamoDB's `PutItem`/`UpdateItem`/`DeleteItem` call.
    ///
    /// Returns the sequence number assigned to the record, or the reason it was
    /// refused: no such stream, no open shard, or an image nesting past
    /// [`MAX_ATTRIBUTE_NESTING_DEPTH`]. Refusing the over-deep image HERE is what
    /// keeps every later walk over a stored image bounded.
    pub fn append_record(
        &mut self,
        stream_arn: &str,
        event: &RecordAppend,
        at: SystemTime,
    ) -> Result<String, AppendRejected> {
        for image in [
            Some(&event.keys),
            event.new_image.as_ref(),
            event.old_image.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let depth = attribute_map_depth(image);
            if depth > MAX_ATTRIBUTE_NESTING_DEPTH {
                return Err(AppendRejected::NestingTooDeep {
                    depth,
                    limit: MAX_ATTRIBUTE_NESTING_DEPTH,
                });
            }
        }

        let sequence_number = format_sequence_number(self.next_sequence_number);
        let stream = self
            .streams
            .get_mut(stream_arn)
            .ok_or(AppendRejected::UnknownStream)?;
        let view_type = stream.stream_view_type;
        let aws_region = stream.aws_region.clone();
        let shard = stream
            .shards
            .iter_mut()
            .rfind(|shard| shard.is_open())
            .ok_or(AppendRejected::NoOpenShard)?;

        let new_image = view_type
            .captures_new_image()
            .then(|| event.new_image.clone())
            .flatten();
        let old_image = view_type
            .captures_old_image()
            .then(|| event.old_image.clone())
            .flatten();
        let size_bytes = record_size_bytes(&event.keys, new_image.as_ref(), old_image.as_ref());

        shard.records.push_back(StoredRecord {
            event_id: event_id(stream_arn, &shard.shard_id, &sequence_number),
            event_name: event.event_name,
            event_version: EVENT_VERSION.to_string(),
            event_source: EVENT_SOURCE.to_string(),
            aws_region,
            sequence_number: sequence_number.clone(),
            // Floored where it is stamped: the member it feeds is reported "rounded
            // down to the closest second" ([`floor_to_second`]), and the stamp is the
            // instant of the WRITE rather than of a later read.
            approximate_creation_date_time: floor_to_second(at),
            written_at: at,
            keys: event.keys.clone(),
            new_image,
            old_image,
            size_bytes,
        });
        // What this stream retains is budgeted, like every other growth site here: the
        // records grow with the writes a caller makes to the table, so the oldest give
        // way once the stream holds more than its ceiling.
        stream.retained_bytes = stream.retained_bytes.saturating_add(size_bytes);
        stream.trim_to_retained_budget();
        self.next_sequence_number += 1;
        Ok(sequence_number)
    }

    /// Close the stream's open shard and open a child shard on it, the way
    /// DynamoDB rolls a shard over as a stream ages.
    ///
    /// Returns the new shard's id, or `None` when the stream is not registered or
    /// has no open shard.
    pub fn roll_shard(&mut self, stream_arn: &str, at: SystemTime) -> Option<String> {
        let (closing_shard_id, ending_sequence_number) = {
            let stream = self.streams.get(stream_arn)?;
            let open = stream.shards.iter().rfind(|shard| shard.is_open())?;
            // A shard that never took a record still needs an ending sequence
            // number of 21..40 digits, so it closes on its own starting number.
            let ending = open
                .records
                .back()
                .map_or_else(|| open.starting_sequence_number.clone(), |record| {
                    record.sequence_number.clone()
                });
            (open.shard_id.clone(), ending)
        };
        let child = self.new_shard(stream_arn, Some(closing_shard_id.clone()), at);
        let child_shard_id = child.shard_id.clone();
        let stream = self.streams.get_mut(stream_arn)?;
        if let Some(closing) = stream
            .shards
            .iter_mut()
            .find(|shard| shard.shard_id == closing_shard_id)
        {
            closing.ending_sequence_number = Some(ending_sequence_number);
        }
        stream.shards.push(child);
        Some(child_shard_id)
    }

    /// Record a shard iterator against the position it names and return the
    /// opaque token the client gets. The token carries no position itself: it is
    /// resolved here, so a client cannot forge or decode one.
    ///
    /// The live table is capped at [`MAX_LIVE_ITERATORS`]: a mint past it retires
    /// the OLDEST live iterator, so a client asking for iterators cannot grow the
    /// store without limit and the token it evicted still answers as the expired
    /// iterator it now is.
    pub fn mint_iterator(
        &mut self,
        stream_arn: &str,
        shard_id: &str,
        next_position: u64,
        at: SystemTime,
    ) -> String {
        let ordinal = self.next_iterator_ordinal;
        self.next_iterator_ordinal += 1;
        let token = opaque_token(stream_arn, shard_id, next_position, ordinal);
        let key = iterator_key(&token);
        self.iterators.insert(
            key.clone(),
            IteratorState {
                stream_arn: stream_arn.to_string(),
                shard_id: shard_id.to_string(),
                next_position,
                issued_at: at,
            },
        );
        self.live_iterator_keys.push_back(key);
        while self.live_iterator_keys.len() > MAX_LIVE_ITERATORS {
            let Some(oldest) = self.live_iterator_keys.pop_front() else {
                break;
            };
            // A token the clock already retired is simply dropped from the order;
            // one that is still live is retired now.
            if self.iterators.remove(&oldest).is_some() {
                self.retire_iterator(oldest);
            }
        }
        token
    }

    /// Remember that the token `key` derives from was an iterator of this
    /// service's, now past its lifetime, bounded by [`MAX_RETIRED_ITERATORS`].
    fn retire_iterator(&mut self, key: String) {
        if self.retired_iterator_keys.insert(key.clone()) {
            self.retired_iterator_order.push_back(key);
        }
        while self.retired_iterator_order.len() > MAX_RETIRED_ITERATORS {
            if let Some(dropped) = self.retired_iterator_order.pop_front() {
                self.retired_iterator_keys.remove(&dropped);
            }
        }
    }

    /// A new shard, opened at the position the stream's counter stands at.
    ///
    /// The shard's `StartingSequenceNumber` is the position the shard OPENS at, and it is
    /// consumed by the shard itself: a record's `SequenceNumber` sorts strictly after the
    /// starting sequence number of the shard that carries it, because the range a shard
    /// advertises opens BEFORE its first record ("The starting sequence number for the
    /// stream records contained within a shard" — `SequenceNumberRange`). That is what lets
    /// a consumer check a record off against the shard's own bounds and resume strictly
    /// after a checkpoint; a record sharing the shard's opening number would order with it
    /// instead.
    fn new_shard(
        &mut self,
        stream_arn: &str,
        parent_shard_id: Option<String>,
        at: SystemTime,
    ) -> StoredShard {
        let ordinal = self.next_shard_ordinal;
        self.next_shard_ordinal += 1;
        let opens_at = self.next_sequence_number;
        self.next_sequence_number += 1;
        StoredShard {
            shard_id: shard_id(stream_arn, ordinal, at),
            parent_shard_id,
            starting_sequence_number: format_sequence_number(opens_at),
            ending_sequence_number: None,
            records: VecDeque::new(),
            trimmed: 0,
        }
    }
}

/// Whether `at` is more than `ttl` before `now`.
fn older_than(at: SystemTime, now: SystemTime, ttl: Duration) -> bool {
    now.duration_since(at).is_ok_and(|age| age > ttl)
}

/// Milliseconds since the Unix epoch, clamped at the epoch for an earlier instant.
#[must_use]
pub fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or_default()
}

/// `at` with its sub-second component dropped: the whole second at or before it, so
/// the value never names a moment that has not happened yet.
///
/// A record's creation instant is reported through
/// `StreamRecord.ApproximateCreationDateTime`, which the model documents as "in UNIX
/// epoch time format and rounded down to the closest second": the member carries
/// whole seconds and nothing finer, so the instant is floored where it is STAMPED and
/// every later read of it agrees.
///
/// An instant BEFORE the epoch is returned unchanged — epoch time does not reach it,
/// and this service stamps a record from the clock, which is long past it.
#[must_use]
pub fn floor_to_second(at: SystemTime) -> SystemTime {
    at.duration_since(UNIX_EPOCH)
        .map_or(at, |since| UNIX_EPOCH + Duration::from_secs(since.as_secs()))
}

/// Every stream ARN of one table starts with this prefix — `.../table/<name>/
/// stream/` — which is what makes the table's own streams a RANGE of the
/// ARN-ordered registry rather than a scan of it. The trailing `/stream/` is part
/// of the prefix, so one table's streams can never be read as another's.
#[must_use]
pub fn stream_arn_prefix(region: &str, table_name: &str) -> String {
    ::engine::emulator_arn(
        DYNAMODB_ARN_NAMESPACE,
        region,
        &format!("{TABLE_RESOURCE_TYPE}/{table_name}{STREAM_RESOURCE_STEP}"),
    )
}

/// The ARN namespace a stream is addressed in: a stream belongs to a DynamoDB
/// table, so it is named inside that service's namespace rather than one of its
/// own.
const DYNAMODB_ARN_NAMESPACE: &str = "dynamodb";

/// The resource type a table is named by inside an ARN's resource field.
const TABLE_RESOURCE_TYPE: &str = "table";

/// The step from a table's resource field to one of that table's streams: a stream's
/// resource field is `table/<name>/stream/<label>`, which is what
/// [`stream_arn_prefix`] mints and what a client reads back off every stream ARN this
/// service reports.
const STREAM_RESOURCE_STEP: &str = "/stream/";

/// Whether a client-supplied value is WRITTEN as the ARN of a DynamoDB stream.
///
/// The form of the identifier, never whether a stream exists under it: a value this
/// says no to — a table's own ARN, an ARN of another service, a value that is not an
/// ARN at all — names no stream in any account or Region, so it can never become
/// found however long a caller waits, while one it says yes to is resolved against
/// the registry and may legitimately be absent.
#[must_use]
pub fn is_stream_arn(value: &str) -> bool {
    // `parse_of` is the one reader a service judges its OWN resource ARNs with: a
    // well-formed ARN whose vendor field is this namespace, in one call.
    ::service_common::arn::parse_of(value, DYNAMODB_ARN_NAMESPACE)
        .is_some_and(|arn| table_stream_resource(arn.resource).is_some())
}

/// The TABLE and the stream LABEL a stream ARN's RESOURCE FIELD names, and `None` for a
/// resource field that names no one stream of one table.
///
/// A stream of a table is addressed by both halves of `table/<name>/stream/<label>`
/// ([`STREAM_RESOURCE_STEP`]) — the label identifies one stream of that table, "the
/// combination of … table name, StreamLabel is guaranteed to be unique"
/// (`StreamDescription$StreamLabel`) — so a resource field that stops at the table, or
/// carries a further `/` in either half, addresses something other than one stream of
/// one table.
///
/// The reading [`is_stream_arn`] is written on, and the one THIS service's own
/// identifiers are judged by, so another service holding a caller-supplied value to
/// what it names reads it from here rather than keeping a second copy of the rule
/// (`service-com-amazonaws-lambda-awsgirapiservice`'s `EventSourceArn` is held to it).
#[must_use]
pub fn table_stream_resource(resource: &str) -> Option<(&str, &str)> {
    let named = resource
        .strip_prefix(TABLE_RESOURCE_TYPE)
        .and_then(|rest| rest.strip_prefix('/'))?;
    let (table_name, stream_label) = named.split_once(STREAM_RESOURCE_STEP)?;
    // Both halves name one thing each: a resource field carrying a further `/` in
    // either of them addresses something other than a stream of a table.
    let names_one_each = !table_name.is_empty()
        && !table_name.contains('/')
        && !stream_label.is_empty()
        && !stream_label.contains('/');
    names_one_each.then_some((table_name, stream_label))
}

/// What a request is refused with when a member that carries a stream ARN was given
/// something that is not written as one.
///
/// Wholly fixed text naming the SHAPE the member targets (`StreamArn`), which is why
/// every member of that shape draws the same sentence: it states what kind of
/// identifier was expected, and echoes neither the value submitted nor the member it
/// arrived in.
pub const INVALID_STREAM_ARN: &str = "Invalid StreamArn";

/// What a request is refused with when the member that carries a shard iterator does
/// not name a position this service can read from — a token it never minted, or no
/// token at all.
///
/// The same wholly fixed form as [`INVALID_STREAM_ARN`], naming the SHAPE the member
/// targets (`ShardIterator`): an iterator is opaque, so there is nothing of the value
/// to report and nothing about the position it would have named to disclose.
pub const INVALID_SHARD_ITERATOR: &str = "Invalid ShardIterator";

/// The ARN a stream of this table registered at `at` carries.
fn stream_arn_at(registration: &StreamRegistration, at: SystemTime) -> String {
    format!(
        "{}{}",
        stream_arn_prefix(&registration.region, &registration.table_name),
        iso8601_label(at)
    )
}

/// A stream label: "A timestamp, in ISO 8601 format, for this stream"
/// (`Stream$StreamLabel`), e.g. `2026-10-21T21:58:07.123`.
///
/// Rendered at FIXED millisecond precision — always three fractional-second digits —
/// so every label of every stream has the same form whatever instant it names: the
/// label identifies the stream to the millisecond, and the stream's ARN is built from
/// it, so a label whose width followed its own digits would give one stream an
/// identifier spelt unlike every other's. The width comes from the resolution the
/// identity is minted at, not from the value: `…:07.000` and `…:07.380` are as much
/// labels of their instant as `…:07.123` is.
#[must_use]
pub fn iso8601_label(at: SystemTime) -> String {
    let millis = epoch_millis(at);
    // The sub-second field is written from the instant's own milliseconds rather than
    // taken from the rendering below, which drops a fraction's trailing zeros — and a
    // whole second's fraction entirely.
    let subsecond_millis = millis.rem_euclid(1_000);
    let rendered = ::aws_smithy_types::DateTime::from_millis(millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    // `Format::DateTime` renders RFC 3339 with a `Z` zone; the label AWS emits for
    // a stream is the same instant without it. The calendar (month lengths, leap
    // years) still comes from the library: only the fraction is written here.
    let civil = rendered.strip_suffix('Z').unwrap_or(&rendered);
    let whole_second = civil.split_once('.').map_or(civil, |(seconds, _)| seconds);
    if whole_second.is_empty() {
        // Unreachable for any instant `from_millis` accepts, but a label must
        // never be empty: the stream ARN is built from it.
        return millis.to_string();
    }
    format!("{whole_second}.{subsecond_millis:03}")
}

/// A sequence number for the `n`th record: a 21-digit decimal string, so the
/// fixed width makes the modelled "numeric characters only" ordering total.
#[must_use]
pub fn format_sequence_number(n: u64) -> String {
    (SEQUENCE_NUMBER_BASE + u128::from(n)).to_string()
}

/// Compare two decimal sequence-number strings NUMERICALLY.
///
/// `SequenceNumber` is 21..40 characters of digits, which overflows every fixed
/// integer width, so the comparison is by significant-digit count and then
/// lexicographic.
#[must_use]
pub fn compare_sequence_numbers(left: &str, right: &str) -> std::cmp::Ordering {
    let left = left.trim_start_matches('0');
    let right = right.trim_start_matches('0');
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

/// Whether a client-supplied sequence number is well formed: the model documents
/// `SequenceNumber` as containing "numeric characters only", over the declared
/// length bound of 21..40 characters.
#[must_use]
pub fn is_sequence_number(value: &str) -> bool {
    (21..=40).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// A shard id of the shape AWS emits — `shardId-<20 digits>-<8 hex>`, 37
/// characters, inside the modelled 28..65 bound. The digits are the shard's
/// creation instant and the suffix is derived from the stream ARN and the shard's
/// ordinal, so the id is unique and reproducible rather than random.
fn shard_id(stream_arn: &str, ordinal: u64, at: SystemTime) -> String {
    let digest = digest_hex(&[stream_arn.as_bytes(), &ordinal.to_be_bytes()]);
    format!(
        "shardId-{:020}-{}",
        epoch_millis(at).unsigned_abs(),
        &digest[..8]
    )
}

/// A `Record.eventID`: "A globally unique identifier for the event that was
/// recorded in this stream record." Derived from the record's own coordinates, so
/// it is unique per record and stable across reads.
fn event_id(stream_arn: &str, shard_id: &str, sequence_number: &str) -> String {
    digest_hex(&[
        stream_arn.as_bytes(),
        shard_id.as_bytes(),
        sequence_number.as_bytes(),
    ])[..32]
        .to_string()
}

/// An opaque shard-iterator token, of the form a DynamoDB Streams iterator takes:
/// `<stream ARN>|<format version>|<base64 envelope>`. The leading ARN is what lets a
/// client that holds only an iterator tell which stream it belongs to — the
/// DynamoDB Streams Kinesis adapter reads it straight off the token — while the
/// third segment carries no position at all: it is an envelope over a digest, not an
/// encoding of the input, so a client can neither decode a token nor forge one for a
/// different position. The whole token stays well inside the modelled
/// `ShardIterator` 1..2048 bound, even for the longest stream ARN there can be.
fn opaque_token(stream_arn: &str, shard_id: &str, next_position: u64, ordinal: u64) -> String {
    let seed = digest_bytes(&[
        stream_arn.as_bytes(),
        shard_id.as_bytes(),
        &next_position.to_be_bytes(),
        &ordinal.to_be_bytes(),
    ]);
    let payload = expanded_bytes(&seed, ITERATOR_PAYLOAD_BYTES);
    let mut envelope = Vec::with_capacity(
        ITERATOR_ENVELOPE_PREAMBLE.len() + ITERATOR_PAYLOAD_LENGTH_FIELD.len() + payload.len(),
    );
    envelope.extend_from_slice(&ITERATOR_ENVELOPE_PREAMBLE);
    envelope.extend_from_slice(&ITERATOR_PAYLOAD_LENGTH_FIELD);
    envelope.extend_from_slice(&payload);
    let blob = ::aws_smithy_types::base64::encode(&envelope);
    format!("{stream_arn}|{ITERATOR_TOKEN_VERSION}|{blob}")
}

/// `len` bytes expanded from one 32 byte seed, by digesting the seed together with a
/// block counter.
///
/// The envelope payload has to be long without being guessable or repeating, and it
/// has to be REPRODUCIBLE here rather than random: the same coordinates always mint
/// the same bytes, and two different coordinates never share them, because the seed
/// they expand from already separates them.
fn expanded_bytes(seed: &[u8; 32], len: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(len + 32);
    let mut block: u64 = 0;
    while bytes.len() < len {
        bytes.extend_from_slice(&digest_bytes(&[seed.as_slice(), &block.to_be_bytes()]));
        block += 1;
    }
    bytes.truncate(len);
    bytes
}

/// The store's key for an iterator token: a fixed-size derivation of the WHOLE
/// token.
///
/// The token a client gets is a kilobyte-scale envelope, and this service keeps
/// [`MAX_LIVE_ITERATORS`] live plus [`MAX_RETIRED_ITERATORS`] retired entries, so
/// keying the tables by the token itself would make what they hold a function of the
/// token's length. Keying them by this digest keeps it a function of the caps alone
/// and still resolves a presented token EXACTLY: the key is derived from the token
/// the client sent, so only the token this service minted reaches the entry it
/// minted.
fn iterator_key(token: &str) -> String {
    digest_hex(&[token.as_bytes()])
}

fn digest_hex(parts: &[&[u8]]) -> String {
    hex::encode(digest_bytes(parts))
}

fn digest_bytes(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        // Length-prefixed, so concatenating two different field splits cannot
        // produce the same digest.
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// `StreamRecord.SizeBytes`: "The size of the stream record, in bytes."
///
/// Sized the way DynamoDB documents an item's size — the sum of the lengths of
/// its attribute names and values — over exactly the images the stream captures.
/// The modelled member has `range min 1`, so an empty record still sizes as 1.
fn record_size_bytes(
    keys: &AttributeMap,
    new_image: Option<&AttributeMap>,
    old_image: Option<&AttributeMap>,
) -> i64 {
    let total = attribute_map_size(keys)
        .saturating_add(new_image.map_or(0, attribute_map_size))
        .saturating_add(old_image.map_or(0, attribute_map_size));
    total.max(1)
}

/// The summed length of an image's attribute names and values.
///
/// ITERATIVE, over an explicit worklist: `AttributeValue::M`/`L` nest, and a
/// recursive walk over an attacker-chosen nesting depth overflows the thread
/// stack — which in Rust aborts the process instead of unwinding. The worklist
/// grows on the heap with the image's own size, and
/// [`MAX_ATTRIBUTE_NESTING_DEPTH`] already bounds what can be stored.
fn attribute_map_size(map: &AttributeMap) -> i64 {
    let mut total: i64 = 0;
    let mut worklist: Vec<&AttributeValue> = Vec::with_capacity(map.len());
    for (name, value) in map {
        total = total.saturating_add(name.len() as i64);
        worklist.push(value);
    }
    while let Some(value) = worklist.pop() {
        let size = match value {
            AttributeValue::S(s) | AttributeValue::N(s) => s.len() as i64,
            AttributeValue::B(b) => b.len() as i64,
            AttributeValue::Ss(items) | AttributeValue::Ns(items) => {
                items.iter().map(|item| item.len() as i64).sum()
            }
            AttributeValue::Bs(items) => items.iter().map(|item| item.len() as i64).sum(),
            AttributeValue::M(inner) => {
                let mut names: i64 = 0;
                for (name, nested) in inner {
                    names = names.saturating_add(name.len() as i64);
                    worklist.push(nested);
                }
                names
            }
            AttributeValue::L(items) => {
                worklist.extend(items.iter());
                0
            }
            // A null or boolean attribute is one byte of value.
            AttributeValue::Null(_) | AttributeValue::Bool(_) => 1,
        };
        total = total.saturating_add(size);
    }
    total
}

/// How deeply an image nests: the image itself is level 1, the members of a map
/// or list attribute inside it are level 2, and so on. `0` for an empty image.
///
/// ITERATIVE for the same reason [`attribute_map_size`] is: this is the check that
/// refuses an over-deep image, so it must never be the thing that overflows.
#[must_use]
pub fn attribute_map_depth(map: &AttributeMap) -> usize {
    let mut worklist: Vec<(&AttributeValue, usize)> =
        map.values().map(|value| (value, 1usize)).collect();
    let mut deepest = if worklist.is_empty() { 0 } else { 1 };
    while let Some((value, depth)) = worklist.pop() {
        deepest = deepest.max(depth);
        match value {
            AttributeValue::M(inner) => {
                worklist.extend(inner.values().map(|nested| (nested, depth + 1)));
            }
            AttributeValue::L(items) => {
                worklist.extend(items.iter().map(|nested| (nested, depth + 1)));
            }
            _ => {}
        }
    }
    deepest
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDynamodbstreamsDynamodbstreams20120810State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDynamodbstreamsDynamodbstreams20120810State>)
}
