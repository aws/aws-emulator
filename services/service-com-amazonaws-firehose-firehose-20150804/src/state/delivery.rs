// SPDX-License-Identifier: CC0-1.0

//! Delivery of a Firehose stream's buffer to its destination.
//!
//! `PutRecord` / `PutRecordBatch` only ACCEPT data: "Firehose buffers records before
//! delivering them to the destination" (`PutRecord`). This module is the other half —
//! what takes the records a stream's buffer holds, once that buffer is due, and writes
//! them into the Amazon S3 location the stream's CURRENT destination configuration names.
//! The one object store in this emulator is the Amazon S3 service's own, so a delivered
//! object is written there and is then read back with `ListObjectsV2` / `GetObject` like
//! any other object.
//!
//! What a delivery is made of, each part read from the destination configuration as it
//! stands at the moment of the flush (so an `UpdateDestination` that repoints `BucketARN`
//! sends every later delivery to the new bucket):
//!
//! * WHEN — `BufferingHints`, whose `SizeInMBs` and `IntervalInSeconds` decide when a
//!   buffer is flushed ([`delivery_window`]).
//! * WHERE — the bucket of `BucketARN`, under `Prefix` with the automatic
//!   `YYYY/MM/DD/HH` expansion ("The \"YYYY/MM/DD/HH\" time format prefix is
//!   automatically used for delivered Amazon S3 files" — `Prefix`), or under the prefix
//!   the enabled `DynamicPartitioningConfiguration`'s partition keys derive
//!   ([`expand_prefix`]), and `ErrorOutputPrefix` for a record whose partition keys could
//!   not be derived ("A prefix that Firehose evaluates and adds to failed records before
//!   writing them to S3" — `ErrorOutputPrefix`).
//! * IN WHICH ZONE — every time field of that location, and of the object's own name, is
//!   the delivery's instant as the zone `CustomTimeZone` names reads it ("The time zone
//!   you prefer. UTC is the default." — [`time_zone`]).
//! * WHAT — the buffered records' data, concatenated in arrival order, encoded as
//!   `CompressionFormat` asks ([`compressed`]).
//! * UNDER WHICH KEY — the `AWSKMSKeyARN` of `EncryptionConfiguration`'s
//!   `KMSEncryptionConfig`, "the encryption key" the destination's objects are encrypted at
//!   rest with ([`kms_key_arn`], [`object_encryption`]).
//! * UNDER WHOSE PERMISSIONS — the `RoleARN` the destination configuration names, which is
//!   the role Firehose makes the write under, so the write happens only where the
//!   permissions in force on that role allow it ([`withheld_writes`]). A delivery the
//!   destination did not take is reported through the destination's own
//!   `CloudWatchLoggingOptions` ([`destination_logging`]) and the records stay buffered for
//!   a later attempt, which is all a producer can observe of it.
//!
//! ## An HTTP endpoint destination
//!
//! A destination that is an HTTP endpoint is not delivered to by writing an object: the
//! buffer is POSTed to the URL the destination's `EndpointConfiguration` names, as the
//! delivery protocol `HttpEndpointConfiguration$Url` points at documents ("review and
//! follow the instructions in the Appendix - HTTP Endpoint Delivery Request and Response
//! Specifications"). The request carries a JSON body of `requestId`, `timestamp` and a
//! `records` array of base64 `data` items, the `RequestConfiguration`'s `ContentEncoding`
//! and `CommonAttributes`, and the `AccessKey` the configuration carries —
//! [`post_to_endpoint`]. A buffer the endpoint does not acknowledge is attempted again for
//! as long as `RetryOptions` allows and is then parked in the destination's Amazon S3
//! backup ([`backup_object`]), so nothing a put accepted is lost.
//!
//! The POST is sent through the IN-PROCESS transport of the emulator this delivery runs
//! within (the connector of `smithy_server_runtime::OperationContext::callback`, handed to
//! [`deliver_due`]): the endpoints this emulator can deliver to are the ones it serves
//! itself, so a delivery opens no socket and reaches nothing outside this process.
//!
//! ## A Splunk destination
//!
//! A destination in Splunk is delivered to the same way: the buffer is POSTed to the HTTP
//! Event Collector `HECEndpoint` names, and the events the collector did not take are parked
//! in the Amazon S3 location its `S3Configuration` names once `RetryOptions` is spent —
//! [`splunk`].
//!
//! ## The clock
//!
//! A flush is a clock-owed change like any other in this crate: [`take_due`] applies only
//! what the stored due instant has ALREADY made due, so a caller reading the destination
//! before that instant sees the data still buffered, and no operation sleeps for it.
//! Delivery happens without any further Firehose call, because that is what a producer
//! observes — so [`drive_delivery_clock`] starts one in-process task per store that waits
//! for the earliest due buffer, delivers it and stands down once nothing is owed.
//!
//! ## Lock order
//!
//! This service's store is locked, the records are TAKEN out of it, and the lock is
//! released before the object store is locked: the two locks are never held at once, and
//! the Amazon S3 service never reaches into this one, so a cycle between them is
//! unreachable.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ::aws_smithy_runtime_api::client::http::{HttpConnector, SharedHttpConnector};

use crate::ops::types::{
    BufferingHints, EncryptionConfiguration, ProcessingConfiguration, S3DestinationConfiguration,
};
use crate::state::cloudwatch_logs_pipeline::{CloudWatchLogsPipeline, Processed};
use crate::state::delivery::destination_logging::{
    DeliveryErrorReport, DestinationError, DestinationLogging, S3_ACCESS_DENIED,
    S3_NO_SUCH_BUCKET,
};
use crate::state::delivery::time_zone::DeliveryTimeZone;
use crate::state::{
    service_state, ComAmazonawsFirehoseFirehose20150804State, DeliveryStream,
    DestinationConfigKind, StoredRecord,
};

/// Where a delivery the destination did not take is reported.
pub mod destination_logging;
/// Converting a buffer's records to the format a `DataFormatConversionConfiguration` asks
/// for, against the column information of the Glue table it binds.
pub mod format_conversion;
/// The delivery of a buffer to a destination in Splunk.
pub mod splunk;
/// The zone a destination's time fields are rendered in.
pub mod time_zone;

/// The action an object write is authorized under, which is the one the destination's role
/// has to allow for a delivery to be made: Amazon S3's own write of an object.
const S3_PUT_OBJECT_ACTION: &str = "s3:PutObject";

/// The media type every delivered object is stored with.
///
/// Firehose writes the records' own bytes, whose type it does not interpret and which it
/// may also have compressed, so a delivered object carries the media type of an opaque byte
/// stream; what the bytes were encoded with is stated by the object's content coding
/// instead ([`content_encoding`]).
const DELIVERED_CONTENT_TYPE: &str = "application/octet-stream";

/// The vendor field of a Firehose stream ARN, which is what the Region a delivery asks its
/// authorization question in is read out of ([`delivery_region`]).
const FIREHOSE_ARN_VENDOR: &str = "firehose";

/// How a delivery reaches the OTHER services it needs: the in-process transport an HTTP
/// endpoint destination's buffer is POSTed over, and the emulator the delivery puts its
/// authorization question to IAM through.
///
/// Both are the emulator this delivery runs within ([`DeliveryHop::through`]), so a delivery
/// opens no socket and reaches nothing outside this process. A hop built over a transport
/// ALONE ([`DeliveryHop::over_transport`]) can ask no one about a role, which is read the way
/// every other unanswerable authorization question here is: nothing is withheld, exactly as
/// `service_common::iam_role::role_withholds_action` reads a hop that could not be made.
#[derive(Debug, Clone)]
pub struct DeliveryHop {
    transport: SharedHttpConnector,
    emulator: Option<::smithy_server_runtime::EmbeddedEmulator>,
}

impl DeliveryHop {
    /// The hop of the emulator a request is running within: its own in-process transport,
    /// and the emulator itself for the questions a delivery has to ask another service.
    #[must_use]
    pub fn through(emulator: &::smithy_server_runtime::EmbeddedEmulator) -> Self {
        Self {
            transport: emulator.http_connector(),
            emulator: Some(emulator.clone()),
        }
    }

    /// A hop over `transport` and nothing else — for a caller that drives one delivery over a
    /// transport of its own and has no emulator to ask about a role.
    #[must_use]
    pub fn over_transport(transport: SharedHttpConnector) -> Self {
        Self {
            transport,
            emulator: None,
        }
    }

    /// The transport an HTTP endpoint destination is POSTed over.
    fn transport(&self) -> &SharedHttpConnector {
        &self.transport
    }

    /// The emulator a delivery asks IAM about a destination role through, and `None` where
    /// there is none to ask.
    fn emulator(&self) -> Option<&::smithy_server_runtime::EmbeddedEmulator> {
        self.emulator.as_ref()
    }
}

/// Bytes in one MiB: `BufferingHints$SizeInMBs` buffers "to the specified size, in MiBs".
const BYTES_PER_MIB: usize = 1024 * 1024;

/// `BufferingHints$SizeInMBs` when the configuration names none: "The default value is 5."
const DEFAULT_BUFFER_SIZE_IN_MIB: usize = 5;

/// `BufferingHints$IntervalInSeconds` when the configuration names none: "The default
/// value is 300."
const DEFAULT_BUFFER_INTERVAL: Duration = Duration::from_secs(300);

/// How long after a stream's buffer opens — the instant the oldest record it holds landed
/// — the delivery comes due once the buffer holds at least `BufferingHints$SizeInMBs` of
/// data, and the shortest window any buffer is held for.
///
/// The size and the interval are two conditions on one buffer and the FIRST one satisfied
/// flushes it ("Firehose buffers data up to 5 MB or for 5 minutes, whichever condition is
/// satisfied first" — `CreateDeliveryStream`), so a buffer that has reached its size is
/// delivered without waiting out the rest of its interval. A CHOSEN span, and never zero:
/// `IntervalInSeconds` admits 0, and a zero window would hand a caller a delivery its own
/// put had not yet returned from.
pub const FULL_BUFFER_DELIVERY_SPAN: Duration = Duration::from_millis(250);

/// `HttpEndpointRetryOptions$DurationInSeconds` when the configuration names none, and the
/// same span for every other destination whose buffer is attempted against an endpoint and
/// whose own `RetryOptions` member declares no default either ([`splunk`]).
///
/// A CHOSEN span: the member states what the duration MEANS ("the total amount of time
/// that Firehose spends on retries. This duration starts after the initial attempt to send
/// data to the custom destination via HTTPS endpoint fails") but declares no default, so
/// this is the emulator's, kept long enough that a destination which is briefly not
/// answering is retried rather than backed up.
const DEFAULT_ENDPOINT_RETRY_DURATION: Duration = Duration::from_secs(300);

/// How long a stream whose buffer could not be delivered at all waits before the delivery
/// is attempted again.
///
/// A CHOSEN span: AWS documents a retry duration only for the destinations whose own
/// `RetryOptions` carry one, and never for how quickly the attempt is repeated. Its job
/// here is that a destination this emulator cannot write to — a `BucketARN` naming a
/// bucket that was never created — costs one attempt per span rather than a spin, while
/// the records stay buffered for the documented [`crate::state::RECORD_RETENTION`] "as it
/// tries to send the records to the destination".
pub const DELIVERY_RETRY_SPAN: Duration = Duration::from_millis(500);

/// The shortest a delivery pump wake-up waits, so a buffer already past its due instant
/// still costs a yield rather than a CPU-bound spin.
const MIN_DELIVERY_WAIT: Duration = Duration::from_millis(5);

/// The longest a delivery pump wake-up waits, so a pump re-reads the store — which a
/// `PutRecord` or an `UpdateDestination` may have changed since — at least this often.
const MAX_DELIVERY_WAIT: Duration = Duration::from_millis(500);

/// How many times one delivery pump wakes up before it stands down.
///
/// A backstop, not a schedule: it bounds the work one put can start, and whatever is still
/// owed when it is reached is delivered by the next pump (every put starts one if none is
/// running).
const MAX_DELIVERY_WAKE_UPS: usize = 2_000;

/// The jq program of one flush gets this long to run over the whole buffer.
///
/// A BACKSTOP, not the bound: a program this emulator runs terminates by construction
/// ([`extract_partition_keys`]), so this is what catches an engine that nevertheless does
/// not come back — the records it did not produce keys for are delivered to the error
/// output prefix, which is where a record whose partition keys could not be determined
/// goes.
const METADATA_EXTRACTION_BUDGET: Duration = Duration::from_secs(2);

/// How often the delivery looks for the result of a running program while waiting out
/// [`METADATA_EXTRACTION_BUDGET`] — an await point, so nothing of the runtime is held
/// while a program runs.
const METADATA_EXTRACTION_POLL: Duration = Duration::from_millis(5);

/// The name the thread a metadata-extraction program runs on carries, so one that outlives
/// its budget is identifiable in a dump.
const METADATA_EXTRACTION_THREAD: &str = "firehose-metadata-extraction";

/// The stack the thread a metadata-extraction program runs on gets.
///
/// The interpreter descends with the program's NESTING, and nothing it runs recurses
/// ([`MAX_PROGRAM_DEPTH`], enforced on the text before the program is parsed), so this is
/// a stack sized for an enforced depth bound rather than a hope.
const METADATA_EXTRACTION_STACK_BYTES: usize = 16 * 1024 * 1024;

/// The deepest a metadata-extraction program may nest, counted over its text BEFORE it is
/// parsed — so no descent over the program has begun when one past this is refused.
///
/// This is the depth bound the interpreter's own descent, and the stack it runs on, are
/// sized for.
const MAX_PROGRAM_DEPTH: usize = 32;

/// The most tokens a metadata-extraction program may carry, counted over its text before
/// it is parsed. Together with [`MAX_PROGRAM_CONSTRUCTIONS`] this is what bounds the VALUE
/// one run can build — see [`admissible_program`].
const MAX_PROGRAM_TOKENS: usize = 128;

/// How many container CONSTRUCTIONS (`{…}`, `[…]`) a metadata-extraction program may
/// carry — indexing and slicing (`.[…]`) are not constructions and are not counted.
///
/// One, because a `MetadataExtractionQuery` produces ONE object of partition keys, and
/// because construction is the only way a jq program makes a value LARGER than the one it
/// was given: a construction holding `n` copies of its input grows it `n`-fold, so a chain
/// of `k` of them grows it `n^k`-fold. With `k` fixed at one and `n` bounded by the token
/// count, the largest value a run can build is bounded — see [`admissible_program`].
const MAX_PROGRAM_CONSTRUCTIONS: usize = 1;

/// How many ITERATIONS (`.a[]`, the empty-bracket form) a metadata-extraction program may
/// carry.
///
/// An iteration is the one admitted form that is a GENERATOR: it yields one output per
/// element of the value it is applied to, and everything after it in a pipe is re-applied
/// once per output — so two of them in one program multiply, `n` elements by `m`, and a
/// third cubes it, which is exactly the growth [`MAX_PROGRAM_CONSTRUCTIONS`] exists to
/// bound. One is admitted, because reading a partition key out of a list needs one and its
/// outputs together are no larger than the record they came out of; a second is refused.
const MAX_PROGRAM_ITERATIONS: usize = 1;

/// The largest record a metadata-extraction program is run over.
///
/// An emulator resource budget, NOT an AWS limit: it is the other half of the memory bound
/// above, since what a run can build is a multiple of what it is given. A record past it
/// has no partition keys derived and goes to the error output, like any other record whose
/// extraction did not succeed. Generous for the JSON document a partition key is read out
/// of, while keeping one run's values at [`MAX_PROGRAM_TOKENS`] times this.
const MAX_EXTRACTED_RECORD_BYTES: usize = 128 * 1024;

/// How many metadata-extraction programs may be running at once in this process.
///
/// A program this emulator runs terminates by construction, so this is not what makes the
/// feature safe — it is what keeps a burst of due buffers from starting an unbounded
/// number of threads at once. The permit is released by the WAITER, on the run's
/// completion AND on the backstop budget expiring, so a run that does not come back cannot
/// hold one.
const MAX_RUNNING_METADATA_EXTRACTIONS: usize = 4;

/// How many runs may be ABANDONED — their backstop budget reached while the engine was
/// still inside one of them — in the life of this process, ever.
///
/// A run this emulator starts terminates by construction, so reaching this means an
/// assumption about the engine is wrong. Past it no program is run at all and every record
/// of a dynamically partitioned stream goes to the error output: the feature fails closed
/// rather than letting a caller start one abandoned thread per put.
const MAX_ABANDONED_EXTRACTIONS: usize = 4;

/// How many programs that overran their budget are remembered. See [`overran_programs`].
const MAX_REMEMBERED_OVERRUNS: usize = 64;

/// The most partition keys one record's extracted metadata may carry.
///
/// An emulator output budget, not an AWS limit: the keys are what a client's own program
/// produced, and they are held in this process and interpolated into an object key, so
/// their number and size are bounded like every other caller-driven growth here.
const MAX_PARTITION_KEYS: usize = 32;

/// The longest one partition key's name or value may be, in bytes — the other half of the
/// budget [`MAX_PARTITION_KEYS`] states. Comfortably inside what an S3 object key can hold
/// (1,024 bytes), so a partition this accepts can still be written.
const MAX_PARTITION_VALUE_BYTES: usize = 256;

/// How many due deliveries ONE call performs before leaving the rest to the next one.
///
/// A store can hold thousands of streams, so without this one put would do the work of
/// every buffer that happens to be due at that instant. What this leaves behind stays
/// buffered and is delivered by the pump's next wake-up.
const MAX_DELIVERIES_PER_CALL: usize = 64;

/// How many metadata-extraction programs are running right now. See
/// [`MAX_RUNNING_METADATA_EXTRACTIONS`].
static RUNNING_METADATA_EXTRACTIONS: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// How many runs have been abandoned in the life of this process. See
/// [`MAX_ABANDONED_EXTRACTIONS`].
static ABANDONED_METADATA_EXTRACTIONS: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// The programs that have already overrun [`METADATA_EXTRACTION_BUDGET`] once, which are
/// never run again: one that did not finish will not finish, and running it per flush would
/// turn one bad configuration into a thread per put. Bounded by
/// [`MAX_REMEMBERED_OVERRUNS`], and each program is at most the modelled 5120 characters.
fn overran_programs() -> &'static ::std::sync::Mutex<::std::collections::BTreeSet<String>> {
    static OVERRAN: ::std::sync::Mutex<::std::collections::BTreeSet<String>> =
        ::std::sync::Mutex::new(::std::collections::BTreeSet::new());
    &OVERRAN
}

/// Whether `query` has already overrun its budget once.
fn has_overrun(query: &str) -> bool {
    overran_programs()
        .lock()
        .unwrap_or_else(::std::sync::PoisonError::into_inner)
        .contains(query)
}

/// Remember that `query` overran its budget, while there is room to.
fn remember_overrun(query: String) {
    let mut remembered = overran_programs()
        .lock()
        .unwrap_or_else(::std::sync::PoisonError::into_inner);
    if remembered.len() < MAX_REMEMBERED_OVERRUNS {
        remembered.insert(query);
    }
}

/// The right to run ONE metadata-extraction program, released by the WAITER — when the
/// run comes back, and also when the backstop budget expires without it. So a run that
/// does not come back cannot hold a permit, and the concurrency limit can never be used up
/// permanently.
struct ExtractionPermit;

impl ExtractionPermit {
    /// Take a permit, or `None` when [`MAX_RUNNING_METADATA_EXTRACTIONS`] are already out.
    fn take() -> Option<Self> {
        use ::std::sync::atomic::Ordering;
        let mut running = RUNNING_METADATA_EXTRACTIONS.load(Ordering::Acquire);
        loop {
            if running >= MAX_RUNNING_METADATA_EXTRACTIONS {
                return None;
            }
            match RUNNING_METADATA_EXTRACTIONS.compare_exchange_weak(
                running,
                running + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Self),
                Err(held) => running = held,
            }
        }
    }
}

impl Drop for ExtractionPermit {
    fn drop(&mut self) {
        RUNNING_METADATA_EXTRACTIONS.fetch_sub(1, ::std::sync::atomic::Ordering::AcqRel);
    }
}

/// Charge one ABANDONED run against [`MAX_ABANDONED_EXTRACTIONS`], answering whether it
/// fitted — `false` once this process has abandoned as many as it ever will, after which
/// no program is run at all.
fn charge_abandoned_extraction() -> bool {
    ABANDONED_METADATA_EXTRACTIONS.fetch_add(1, ::std::sync::atomic::Ordering::AcqRel)
        < MAX_ABANDONED_EXTRACTIONS
}

/// Whether this process has already abandoned [`MAX_ABANDONED_EXTRACTIONS`] runs.
fn abandoned_budget_spent() -> bool {
    ABANDONED_METADATA_EXTRACTIONS.load(::std::sync::atomic::Ordering::Acquire)
        >= MAX_ABANDONED_EXTRACTIONS
}

/// The `ProcessorType` whose parameters carry the jq program that derives a record's
/// partition keys.
///
/// `pub(crate)` because the rule about the parameters a processor of this type cannot run
/// without is stated beside the other processor rules in the parent module, over this one
/// name rather than a second spelling of it.
pub(crate) const METADATA_EXTRACTION_PROCESSOR: &str = "MetadataExtraction";

/// The `ProcessorType` that asks for a delimiter between the records of a delivered
/// object: "If you want to add a new line delimiter between records in objects that are
/// delivered to Amazon S3, choose `AppendDelimiterToRecord` as a processor type. You don't
/// have to put a processor parameter when you select `AppendDelimiterToRecord`"
/// (`Processor`).
const APPEND_DELIMITER_PROCESSOR: &str = "AppendDelimiterToRecord";

/// The `ProcessorParameterName` carrying the jq program of a `MetadataExtraction`
/// processor. `pub(crate)` for the same reason as [`METADATA_EXTRACTION_PROCESSOR`].
pub(crate) const METADATA_EXTRACTION_QUERY_PARAMETER: &str = "MetadataExtractionQuery";

/// The delimiter `AppendDelimiterToRecord` adds between the records of one object: the
/// new line that processor exists to add, and the one `PutRecordBatch` names as the way
/// "to disambiguate the data blobs at the destination".
const RECORD_DELIMITER: &[u8] = b"\n";

/// How a record Firehose could not place is reported in the error output.
///
/// A record that does not reach the destination is not dropped: it is written under the
/// destination's `ErrorOutputPrefix`, wrapped in the error-output document Firehose writes
/// for a failed record — [`error_envelope`] — so a consumer of that prefix can tell what
/// failed and still has the record's own bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DeliveryFailure {
    /// What the `firehose:error-output-type` namespace of an `ErrorOutputPrefix` expands
    /// to for this failure, and the prefix a destination configuring none gets.
    output_type: &'static str,
    /// The failure code the error-output document names, under the member
    /// [`Self::reported_member_names`] gives it.
    error_code: &'static str,
    /// The failure message the error-output document names, under the member
    /// [`Self::reported_member_names`] gives it. An example, not a wire contract: what a
    /// consumer keys off is the code beside it.
    error_message: &'static str,
}

impl DeliveryFailure {
    /// The two members this failure's error-output document carries its code and its message
    /// under.
    ///
    /// A record the destination's DATA FORMAT CONVERSION failed on is reported under
    /// `lastErrorCode` / `lastErrorMessage` — the conversion's own error output names the
    /// LAST attempt's failure beside the Data Catalog table it was made against — and every
    /// other failure under `errorCode` / `errorMessage`. Keyed off the failure's own output
    /// type, so the two conversion failures cannot drift apart from each other.
    fn reported_member_names(self) -> (&'static str, &'static str) {
        if self.output_type == FORMAT_CONVERSION_FAILED_OUTPUT_TYPE {
            ("lastErrorCode", "lastErrorMessage")
        } else {
            ("errorCode", "errorMessage")
        }
    }
}

/// The failure of a record whose PARTITION KEYS could not be determined: the
/// `MetadataExtraction` program derived none for it (unreadable JSON, a program that does
/// not compile, an output that is not an object of scalars), or the destination's prefix
/// names a key nothing produced — including a `partitionKeyFromLambda` key, which only a
/// Lambda data transformation supplies and this emulator runs none.
const METADATA_EXTRACTION_FAILED: DeliveryFailure = DeliveryFailure {
    output_type: "metadata-extraction-failed",
    error_code: "DynamicPartitioning.MetadataExtractionFailed",
    error_message: "the partition keys of this record could not be determined",
};

/// What the `firehose:error-output-type` namespace expands to for a record an HTTP endpoint
/// destination did not take, and the prefix its backup gets when the configuration names
/// no `ErrorOutputPrefix`.
const HTTP_ENDPOINT_FAILED_OUTPUT_TYPE: &str = "http-endpoint-failed";

/// The failure of a record the destination's own DATA PROCESSING could not read: a record
/// that is not framed in the format an enabled `Decompression` processor inflates, or whose
/// inflated bytes are not the document an enabled `CloudWatchLogProcessing` processor
/// extracts the log events out of ([`crate::state::cloudwatch_logs_pipeline`]).
///
/// "Enables or disables data processing" (`ProcessingConfiguration$Enabled`) is what puts
/// those processors in force, so this failure exists only for a destination that enabled
/// them; a record of one is not dropped either way, since the error output carries it with
/// its own bytes.
const PROCESSING_FAILED: DeliveryFailure = DeliveryFailure {
    output_type: "processing-failed",
    error_code: "DataProcessing.ProcessingFailed",
    error_message: "this record is not what the destination's data processing reads",
};

/// The failure of a record the destination's own DATA FORMAT CONVERSION could not place:
/// with a `DataFormatConversionConfiguration` in force the records are deserialized from
/// JSON into the columns of the Glue table the configuration binds, and a record that is
/// not one JSON object of values those columns hold is not a row that conversion can write
/// ([`format_conversion`]).
///
/// Such a record is not dropped and does not stop its well-formed siblings from being
/// delivered: it is written under the destination's `ErrorOutputPrefix` with the record's
/// own bytes, which is what makes a schema mismatch something a consumer of that prefix can
/// both see and re-drive.
///
/// Every record of this class carries a code of its OWN — [`CONVERSION_PARSE_ERROR`] or
/// [`CONVERSION_MALFORMED_DATA`], as [`conversion_fault`] reads it off the conversion's own
/// verdict for that record — so the code here is the class's, which stands only for a record
/// the conversion reported no verdict of its own for.
const FORMAT_CONVERSION_FAILED: DeliveryFailure = DeliveryFailure {
    output_type: FORMAT_CONVERSION_FAILED_OUTPUT_TYPE,
    error_code: CONVERSION_PARSE_ERROR,
    error_message: "this record could not be deserialized into the columns of the destination's \
                    schema",
};

/// The conversion error code of a record whose bytes the conversion could not READ as the
/// JSON its deserializer deserializes.
const CONVERSION_PARSE_ERROR: &str = "DataFormatConversion.ParseError";

/// The conversion error code of a record the conversion READ but whose values the bound Glue
/// columns do not hold.
///
/// The class a consumer of the error output tells a record that does not MATCH the declared
/// schema from one that is not parseable JSON at all by: the two are different faults in the
/// producer, so the error output names them apart.
const CONVERSION_MALFORMED_DATA: &str = "DataFormatConversion.MalformedData";

/// What failed for ONE record, where the failure is about that record's own data: the code
/// its error-output document names and what that document says.
///
/// A failure class states one code for every record of a group ([`DeliveryFailure`]); a
/// conversion, which reads each record against the bound columns on its own, states this per
/// record — so one error-output object carries a record the conversion could not read beside
/// one whose values the columns do not hold, each under its own code.
#[derive(Debug, Clone)]
struct RecordFault {
    error_code: &'static str,
    message: String,
}

/// The fault one record's conversion verdict states: the code of its class and what failed
/// within it.
fn conversion_fault(rejection: &format_conversion::Rejection) -> RecordFault {
    let error_code = match rejection {
        format_conversion::Rejection::Unparseable(_) => CONVERSION_PARSE_ERROR,
        format_conversion::Rejection::Malformed(_) => CONVERSION_MALFORMED_DATA,
    };
    RecordFault {
        error_code,
        message: rejection.fault().to_string(),
    }
}

/// The failure of a record whose destination converts its data format but whose bound
/// SCHEMA could not be read at all: the Data Catalog holds no such table, or the table
/// carries no column information, so there are no columns to convert against
/// (`SchemaConfiguration` "specifies the Amazon Web Services Glue table that contains the
/// column information that constitutes your data schema").
const FORMAT_CONVERSION_SCHEMA_MISSING: DeliveryFailure = DeliveryFailure {
    output_type: FORMAT_CONVERSION_FAILED_OUTPUT_TYPE,
    error_code: "DataFormatConversion.InvalidSchema",
    error_message: "the schema the destination converts to could not be read from the Data \
                    Catalog",
};

/// What the `firehose:error-output-type` namespace expands to for a record the destination's
/// data format conversion could not place, and the prefix those records get when the
/// destination configures no `ErrorOutputPrefix`.
const FORMAT_CONVERSION_FAILED_OUTPUT_TYPE: &str = "format-conversion-failed";

/// The failure of a buffer the HTTP endpoint ANSWERED, but not with a success status — the
/// destination raised rather than acknowledged the delivery.
const HTTP_ENDPOINT_DESTINATION_EXCEPTION: DeliveryFailure = DeliveryFailure {
    output_type: HTTP_ENDPOINT_FAILED_OUTPUT_TYPE,
    error_code: "HttpEndpoint.DestinationException",
    error_message: "the HTTP endpoint destination did not acknowledge the delivery",
};

/// The failure of a buffer the HTTP endpoint answered with a SUCCESS status, but not with
/// the acknowledgment the delivery protocol asks for: the answer is not one JSON document
/// naming the request it answers, or it reports an error of its own in that document.
const HTTP_ENDPOINT_INVALID_RESPONSE: DeliveryFailure = DeliveryFailure {
    output_type: HTTP_ENDPOINT_FAILED_OUTPUT_TYPE,
    error_code: "HttpEndpoint.InvalidResponseFromDestination",
    error_message: "the HTTP endpoint destination's answer did not acknowledge the delivery",
};

/// The failure of a buffer whose delivery request could not be made at all: nothing this
/// emulator serves answers at that URL, or the configuration does not render a request
/// (a `Url` that is not a URI, an `AccessKey` or a common attribute that is not a header
/// value).
const HTTP_ENDPOINT_CONNECTION_FAILED: DeliveryFailure = DeliveryFailure {
    output_type: HTTP_ENDPOINT_FAILED_OUTPUT_TYPE,
    error_code: "HttpEndpoint.ConnectionFailed",
    error_message: "the HTTP endpoint destination could not be reached",
};

/// The `ContentEncoding` value that asks for the delivery request's body to be gzipped:
/// "Firehose uses the content encoding to compress the body of a request before sending the
/// request to the destination" (`HttpEndpointRequestConfiguration$ContentEncoding`, whose
/// values are `NONE` and `GZIP`).
const GZIP_CONTENT_ENCODING: &str = "GZIP";

/// The `Content-Encoding` a gzipped delivery request carries, the token that encoding is
/// named by on the wire.
const GZIP_CONTENT_ENCODING_TOKEN: &str = "gzip";

/// The version of the delivery request protocol this request is written in, carried on
/// every request the HTTP endpoint delivery protocol describes.
const PROTOCOL_VERSION: &str = "1.0";

/// The header carrying [`PROTOCOL_VERSION`].
const PROTOCOL_VERSION_HEADER: &str = "x-amz-firehose-protocol-version";

/// The header carrying the id of one delivery request — the same value the body's
/// `requestId` carries, so a destination can tie the two together.
const REQUEST_ID_HEADER: &str = "x-amz-firehose-request-id";

/// The header naming the Firehose stream the delivered records came from.
const SOURCE_ARN_HEADER: &str = "x-amz-firehose-source-arn";

/// The header carrying `EndpointConfiguration$AccessKey`: "the access key required for
/// Kinesis Firehose to authenticate with the HTTP endpoint selected as the destination".
const ACCESS_KEY_HEADER: &str = "x-amz-firehose-access-key";

/// The header carrying `RequestConfiguration$CommonAttributes`: "describes the metadata
/// sent to the HTTP endpoint destination".
const COMMON_ATTRIBUTES_HEADER: &str = "x-amz-firehose-common-attributes";

/// The `S3BackupMode` value that sends a copy of EVERY record to the backup location,
/// whichever destination kind carries the mode (`S3BackupMode` `Enabled`,
/// `ElasticsearchS3BackupMode` `AllDocuments`, `SplunkS3BackupMode` `AllEvents`,
/// `HttpEndpointS3BackupMode` / `SnowflakeS3BackupMode` / `IcebergS3BackupMode`
/// `AllData`). Under any other mode the Amazon S3 location takes only the records the
/// primary destination rejected — which for an HTTP endpoint destination is what
/// [`backup_object`] writes there, and for the endpoints this emulator delivers to none of
/// is nothing.
const ALL_RECORDS_BACKUP_MODES: &[&str] = &["Enabled", "AllDocuments", "AllEvents", "AllData"];

/// One Amazon S3 location a stream's buffer is delivered to, with everything the write
/// needs, read off the destination configuration in force at the moment of the flush.
#[derive(Debug, Clone)]
pub struct S3Target {
    /// The bucket named by `BucketARN`, whose resource part IS the bucket name
    /// (`arn:<partition>:s3:::<name>`).
    pub bucket: String,
    /// `BucketARN` itself, verbatim: the resource an authorization question about a write
    /// here names, and the `destination` a reported failure to write it names.
    pub bucket_arn: String,
    /// The destination's `RoleARN` — "the Amazon Resource Name (ARN) of the Amazon Web
    /// Services credentials" (`S3DestinationConfiguration$RoleARN`) — which is the role the
    /// write is made under.
    pub role_arn: String,
    /// Where a delivery this location did not take is reported: the group and stream the
    /// destination's `CloudWatchLoggingOptions` name, and `None` where they report nothing
    /// ([`destination_logging`]).
    pub logging: Option<DestinationLogging>,
    /// `Prefix`, verbatim; the expansion happens per delivered object.
    pub prefix: String,
    /// `ErrorOutputPrefix`, verbatim.
    pub error_output_prefix: String,
    /// `BufferingHints`, which decide when this location's buffer is due.
    pub hints: Option<BufferingHints>,
    /// A `CompressionFormat` value, or `None` for the documented default: "The compression
    /// format. If no value is specified, the default is UNCOMPRESSED."
    pub compression_format: Option<String>,
    /// `FileExtension`: "Specify a file extension. It will override the default file
    /// extension."
    pub file_extension: Option<String>,
    /// The jq program of the destination's `MetadataExtraction` processor, when dynamic
    /// partitioning is enabled for this location and such a processor is configured.
    pub metadata_extraction_query: Option<String>,
    /// Whether the records of one object are separated by [`RECORD_DELIMITER`], which the
    /// `AppendDelimiterToRecord` processor asks for.
    pub append_delimiter: bool,
    /// What the destination's enabled data processing leaves of each record on the way here
    /// ([`crate::state::cloudwatch_logs_pipeline`]). The default — nothing enabled — leaves
    /// every record exactly as the producer sent it.
    pub cloudwatch_logs: CloudWatchLogsPipeline,
    /// The key an object written here is encrypted at rest with: the `AWSKMSKeyARN` of the
    /// destination's `EncryptionConfiguration` ([`kms_key_arn`]), and `None` when the
    /// configuration names none.
    pub kms_key_arn: Option<String>,
    /// The zone every time field of a delivery to this location is rendered in: the one
    /// `CustomTimeZone` names, and UTC where the destination kind carries no such member or
    /// names none ([`DeliveryTimeZone`]).
    pub time_zone: DeliveryTimeZone,
    /// The data format conversion in force for this location — the one its
    /// `DataFormatConversionConfiguration` asks for ([`format_conversion`]) — and `None`
    /// where the records are delivered as the producer's own bytes.
    pub conversion: Option<format_conversion::FormatConversion>,
}

/// One HTTP endpoint destination a stream's buffer is delivered to, with everything the
/// delivery request needs, read off the destination configuration in force at the moment of
/// the flush.
#[derive(Clone)]
pub struct HttpEndpointTarget {
    /// `EndpointConfiguration$Url`, the URL the buffer is POSTed to.
    pub url: String,
    /// `EndpointConfiguration$AccessKey`, carried on the request so the destination can
    /// authenticate the delivery. `None` when the configuration names none or names an
    /// empty one (the member's own length range admits zero).
    pub access_key: Option<String>,
    /// Whether `RequestConfiguration$ContentEncoding` asks for a gzipped body.
    pub gzip: bool,
    /// `RequestConfiguration$CommonAttributes`, in configuration order.
    pub common_attributes: Vec<(String, String)>,
    /// `BufferingHints`, which decide when this endpoint's buffer is due. The same two
    /// parameters as every other destination's, so one window rule serves both
    /// ([`delivery_window`]).
    pub hints: Option<BufferingHints>,
    /// How long the delivery of one buffer is retried before its records are parked in
    /// [`Self::backup`] — `RetryOptions$DurationInSeconds`, or
    /// [`DEFAULT_ENDPOINT_RETRY_DURATION`].
    pub retry_duration: Duration,
    /// The Amazon S3 location the destination's required `S3Configuration` names, where a
    /// record this endpoint never took is parked.
    pub backup: Option<S3Target>,
}

impl ::std::fmt::Debug for HttpEndpointTarget {
    /// Everything this target carries EXCEPT the values the model declares `@sensitive`:
    /// the URL, the access key and the common attributes are the caller's own, so this form
    /// says whether one is configured and never what it is. The derived form would put them
    /// in whatever formatted it.
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("HttpEndpointTarget")
            .field("access_key", &self.access_key.is_some())
            .field("gzip", &self.gzip)
            .field("common_attributes", &self.common_attributes.len())
            .field("hints", &self.hints)
            .field("retry_duration", &self.retry_duration)
            .field("backup", &self.backup)
            .finish_non_exhaustive()
    }
}

/// A stream's buffer, taken out of the store and ready to be written to its destination.
#[derive(Debug)]
pub struct PendingDelivery {
    /// The stream whose buffer this is, which names the delivered objects.
    pub stream: String,
    /// The stream's ARN, which the delivery request to an HTTP endpoint names as the
    /// source the records came from.
    pub source_arn: String,
    /// The stream's `VersionId`, which a delivered object's name carries.
    pub version_id: u64,
    /// The buffered records, in arrival order.
    pub records: Vec<StoredRecord>,
    /// Every Amazon S3 location this buffer goes to.
    pub targets: Vec<S3Target>,
    /// Every HTTP endpoint this buffer is POSTed to.
    pub endpoints: Vec<HttpEndpointTarget>,
    /// Every Amazon OpenSearch Service index this buffer's records are indexed into as
    /// documents.
    pub searches: Vec<SearchTarget>,
    /// Every Splunk HTTP Event Collector this buffer is POSTed to.
    pub splunks: Vec<splunk::SplunkTarget>,
}

/// One object a delivery writes.
#[derive(Debug)]
struct DeliveredObject {
    bucket: String,
    key: String,
    body: Vec<u8>,
    /// The content coding `body` was encoded with, from the destination's
    /// `CompressionFormat` ([`compressed`]), and `None` for a body that carries none.
    content_encoding: Option<String>,
    /// The KMS key this object is stored encrypted with, from the destination's
    /// `EncryptionConfiguration` ([`S3Target::kms_key_arn`]).
    kms_key_arn: Option<String>,
    /// The destination this object belongs to, for the decisions that are the DESTINATION's
    /// rather than this object's.
    destination: ObjectDestination,
}

/// What a delivered object's own DESTINATION decides: whether the write may be made at all,
/// and where a failure to make it is reported.
#[derive(Debug, Clone)]
struct ObjectDestination {
    /// The destination's `BucketARN` ([`S3Target::bucket_arn`]).
    bucket_arn: String,
    /// The destination's `RoleARN` ([`S3Target::role_arn`]).
    role_arn: String,
    /// Where a failure to write this object is reported ([`S3Target::logging`]).
    logging: Option<DestinationLogging>,
}

impl S3Target {
    /// This location as the destination of the objects written to it.
    fn destination(&self) -> ObjectDestination {
        ObjectDestination {
            bucket_arn: self.bucket_arn.clone(),
            role_arn: self.role_arn.clone(),
            logging: self.logging.clone(),
        }
    }
}

/// The size in bytes at which a buffer with these hints is full, and the longest it is
/// held.
fn buffer_limits(hints: Option<&BufferingHints>) -> (usize, Duration) {
    let size = hints
        .and_then(|hints| hints.size_in_m_bs)
        .and_then(|size| usize::try_from(size).ok())
        .unwrap_or(DEFAULT_BUFFER_SIZE_IN_MIB)
        .saturating_mul(BYTES_PER_MIB);
    let interval = hints
        .and_then(|hints| hints.interval_in_seconds)
        .and_then(|interval| u64::try_from(interval).ok())
        .map_or(DEFAULT_BUFFER_INTERVAL, Duration::from_secs);
    (size, interval)
}

/// How long after it opened a buffer holding `buffered_bytes` under these hints is held
/// before its delivery comes due.
///
/// The configured `IntervalInSeconds` is the wait the producer asked for — "buffer incoming
/// data for the specified period of time, in seconds, before delivering it to the
/// destination" (`BufferingHints$IntervalInSeconds`) — so that is how long a buffer still
/// filling is held, and [`DEFAULT_BUFFER_INTERVAL`] is how long one whose configuration
/// names no interval is. A buffer that has reached `SizeInMBs` is due at
/// [`FULL_BUFFER_DELIVERY_SPAN`] instead, whichever interval was configured, because the
/// first of the two conditions satisfied is what flushes a buffer. That span also floors
/// the window: `IntervalInSeconds` admits 0, and a zero window would hand a caller a
/// delivery its own put had not yet returned from.
#[must_use]
pub fn delivery_window(hints: Option<&BufferingHints>, buffered_bytes: usize) -> Duration {
    let (size, interval) = buffer_limits(hints);
    let window = if buffered_bytes >= size {
        FULL_BUFFER_DELIVERY_SPAN
    } else {
        interval
    };
    window.max(FULL_BUFFER_DELIVERY_SPAN)
}

/// The bucket name a `BucketARN` names, or `None` when the value carries none.
///
/// An S3 bucket ARN has neither account nor Region — `arn:<partition>:s3:::<name>`, the
/// form `BucketARN`'s own modelled pattern requires — so the name is the resource part.
pub(crate) fn bucket_name(bucket_arn: &str) -> Option<&str> {
    let name = bucket_arn.rsplit(':').next().unwrap_or_default();
    (!name.is_empty()).then_some(name)
}

/// The Amazon S3 location a plain `S3DestinationConfiguration` names, taking every record
/// as the producer sent it.
///
/// A plain `S3DestinationConfiguration` carries no `ProcessingConfiguration` of its own, and
/// where one appears as another destination's BACKUP it is the source records that are
/// parked there, not the processed ones — so a location read through here runs no pipeline.
/// [`processing_target`] is the primary location of a destination whose own data processing
/// is in force.
fn plain_target(config: &S3DestinationConfiguration) -> Option<S3Target> {
    Some(S3Target {
        bucket: bucket_name(&config.bucket_arn)?.to_string(),
        bucket_arn: config.bucket_arn.clone(),
        role_arn: config.role_arn.clone(),
        logging: DestinationLogging::of(config.cloud_watch_logging_options.as_ref()),
        prefix: config.prefix.clone().unwrap_or_default(),
        error_output_prefix: config.error_output_prefix.clone().unwrap_or_default(),
        hints: config.buffering_hints.clone(),
        compression_format: config.compression_format.clone(),
        file_extension: None,
        metadata_extraction_query: None,
        append_delimiter: false,
        cloudwatch_logs: CloudWatchLogsPipeline::default(),
        kms_key_arn: kms_key_arn(config.encryption_configuration.as_ref()),
        // A plain `S3DestinationConfiguration` carries no `CustomTimeZone` member at all,
        // so a location read through here is named in the default zone.
        time_zone: DeliveryTimeZone::default(),
        // Nor a `DataFormatConversionConfiguration`: only an EXTENDED Amazon S3 destination
        // carries one, so a location read through here delivers the records' own bytes.
        conversion: None,
    })
}

/// The same location, with the data processing `processing` configures in force: the records
/// written here are what that destination's enabled processors leave behind.
fn processing_target(
    config: &S3DestinationConfiguration,
    processing: Option<&ProcessingConfiguration>,
) -> Option<S3Target> {
    Some(S3Target {
        append_delimiter: appends_delimiter(processing),
        cloudwatch_logs: CloudWatchLogsPipeline::of(processing),
        ..plain_target(config)?
    })
}

/// The KMS key an object delivered to a location is encrypted at rest with: the
/// `AWSKMSKeyARN` of its `EncryptionConfiguration`'s `KMSEncryptionConfig`, which is "the
/// Amazon Resource Name (ARN) of the encryption key" the destination is encrypted under
/// (`KMSEncryptionConfig$AWSKMSKeyARN`).
///
/// `None` for a configuration that names no key — one that names `NoEncryptionConfig`
/// ("specifically override existing encryption information to ensure that no encryption is
/// used") and one that names nothing at all, whose documented default is that same no
/// encryption (`ExtendedS3DestinationConfiguration$EncryptionConfiguration`): the write
/// then asks for no encryption of its own, which is what makes the destination bucket's own
/// default encryption configuration decide, exactly as for any other write to it.
fn kms_key_arn(config: Option<&EncryptionConfiguration>) -> Option<String> {
    let key = &config?.kms_encryption_config.as_ref()?.awskms_key_arn;
    (!key.is_empty()).then(|| key.clone())
}

/// The jq program the destination's enabled `MetadataExtraction` processor carries, when
/// dynamic partitioning is enabled for it.
///
/// Dynamic partitioning "creates smaller data sets from the streaming data by partitioning
/// it based on partition keys" (`DynamicPartitioningConfiguration`), and the keys come from
/// the `MetadataExtractionQuery` of the data-processing configuration — so a configuration
/// with the mechanism switched off, or with no such processor, derives no keys and the
/// whole buffer goes to one prefix.
fn metadata_extraction_query(
    dynamic_partitioning_enabled: bool,
    processing: Option<&ProcessingConfiguration>,
) -> Option<String> {
    if !dynamic_partitioning_enabled {
        return None;
    }
    let processing = processing?;
    if processing.enabled != Some(true) {
        return None;
    }
    processing
        .processors
        .iter()
        .flatten()
        .filter(|processor| processor.r#type == METADATA_EXTRACTION_PROCESSOR)
        .flat_map(|processor| processor.parameters.iter().flatten())
        .find(|parameter| parameter.parameter_name == METADATA_EXTRACTION_QUERY_PARAMETER)
        .map(|parameter| parameter.parameter_value.clone())
}

/// Whether the destination's data processing asks for a delimiter between the records of
/// one delivered object.
fn appends_delimiter(processing: Option<&ProcessingConfiguration>) -> bool {
    processing.is_some_and(|processing| {
        processing.enabled == Some(true)
            && processing
                .processors
                .iter()
                .flatten()
                .any(|processor| processor.r#type == APPEND_DELIMITER_PROCESSOR)
    })
}

/// Whether an `S3BackupMode` value sends every record to the backup location.
fn backs_up_all_records(mode: Option<&String>) -> bool {
    mode.is_some_and(|mode| ALL_RECORDS_BACKUP_MODES.contains(&mode.as_str()))
}

/// Every Amazon S3 location a destination configuration delivers EVERY ingested record to.
///
/// * A plain or extended Amazon S3 destination is itself such a location.
/// * An Amazon Redshift destination has one too: "Firehose first delivers data to the S3
///   bucket and then issues an Amazon Redshift COPY command"
///   (`CreateDeliveryStream`), so its `S3Configuration` takes every record on the way.
/// * Every other destination kind keeps its `S3Configuration` for BACKUP, and only the
///   mode that backs up all records ([`ALL_RECORDS_BACKUP_MODES`]) makes it a location
///   every record reaches; under the failed-records-only modes it takes what the primary
///   destination rejected, and this emulator delivers to none of those endpoints, so it has
///   nothing to write there.
/// * A destination that backs up to Amazon S3 as well ([`ALL_RECORDS_BACKUP_MODES`] with an
///   `S3BackupConfiguration`) delivers to that location too.
#[must_use]
pub fn s3_targets(config: &DestinationConfigKind) -> Vec<S3Target> {
    let mut targets = Vec::new();
    match config {
        DestinationConfigKind::S3(config) => targets.extend(plain_target(config)),
        DestinationConfigKind::ExtendedS3(config) => {
            if let Some(name) = bucket_name(&config.bucket_arn) {
                let dynamic_partitioning_enabled = config
                    .dynamic_partitioning_configuration
                    .as_ref()
                    .is_some_and(|partitioning| partitioning.enabled == Some(true));
                targets.push(S3Target {
                    bucket: name.to_string(),
                    bucket_arn: config.bucket_arn.clone(),
                    role_arn: config.role_arn.clone(),
                    logging: DestinationLogging::of(
                        config.cloud_watch_logging_options.as_ref(),
                    ),
                    prefix: config.prefix.clone().unwrap_or_default(),
                    error_output_prefix: config.error_output_prefix.clone().unwrap_or_default(),
                    hints: config.buffering_hints.clone(),
                    compression_format: config.compression_format.clone(),
                    file_extension: config.file_extension.clone(),
                    metadata_extraction_query: metadata_extraction_query(
                        dynamic_partitioning_enabled,
                        config.processing_configuration.as_ref(),
                    ),
                    append_delimiter: appends_delimiter(config.processing_configuration.as_ref()),
                    cloudwatch_logs: CloudWatchLogsPipeline::of(
                        config.processing_configuration.as_ref(),
                    ),
                    kms_key_arn: kms_key_arn(config.encryption_configuration.as_ref()),
                    time_zone: DeliveryTimeZone::of(config.custom_time_zone.as_deref()),
                    conversion: format_conversion::FormatConversion::of(
                        config.data_format_conversion_configuration.as_ref(),
                    ),
                });
            }
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(config.s3_backup_configuration.as_ref().and_then(plain_target));
            }
        }
        DestinationConfigKind::Redshift(config) => {
            // Amazon Redshift's own Amazon S3 location is on the DELIVERY path — "Firehose
            // first delivers data to the S3 bucket and then issues an Amazon Redshift COPY
            // command" — so what lands there is what this destination's processing left.
            targets.extend(processing_target(
                &config.s3_configuration,
                config.processing_configuration.as_ref(),
            ));
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(config.s3_backup_configuration.as_ref().and_then(plain_target));
            }
        }
        DestinationConfigKind::Elasticsearch(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
        DestinationConfigKind::AmazonOpenSearchService(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
        DestinationConfigKind::Splunk(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
        DestinationConfigKind::HttpEndpoint(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
        DestinationConfigKind::Snowflake(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
        DestinationConfigKind::AmazonOpenSearchServerless(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
        DestinationConfigKind::Iceberg(config) => {
            if backs_up_all_records(config.s3_backup_mode.as_ref()) {
                targets.extend(plain_target(&config.s3_configuration));
            }
        }
    }
    targets
}

/// Every Amazon S3 location this stream's destinations deliver to right now.
fn stream_targets(stream: &DeliveryStream) -> Vec<S3Target> {
    stream
        .destinations
        .iter()
        .flat_map(|destination| s3_targets(&destination.config))
        .collect()
}

/// The HTTP endpoint a destination configuration delivers to — the URL its
/// `EndpointConfiguration` names, and nothing for a destination of any other kind.
///
/// "Describes the configuration of the HTTP endpoint destination"
/// (`HttpEndpointDestinationConfiguration`): the endpoint IS the destination, so the
/// buffer goes to it rather than to an Amazon S3 location, and the configuration's
/// required `S3Configuration` is the backup the records a delivery could not place end up
/// in.
#[must_use]
pub fn http_endpoint_targets(config: &DestinationConfigKind) -> Vec<HttpEndpointTarget> {
    let DestinationConfigKind::HttpEndpoint(config) = config else {
        return Vec::new();
    };
    if config.endpoint_configuration.url.is_empty() {
        return Vec::new();
    }
    let request = config.request_configuration.as_ref();
    vec![HttpEndpointTarget {
        url: config.endpoint_configuration.url.clone(),
        access_key: config
            .endpoint_configuration
            .access_key
            .clone()
            .filter(|key| !key.is_empty()),
        gzip: request
            .and_then(|request| request.content_encoding.as_deref())
            .is_some_and(|encoding| encoding == GZIP_CONTENT_ENCODING),
        common_attributes: request
            .and_then(|request| request.common_attributes.as_ref())
            .map(|attributes| {
                attributes
                    .iter()
                    .map(|attribute| {
                        (
                            attribute.attribute_name.clone(),
                            attribute.attribute_value.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        // The endpoint's own buffering parameters are the same two every other
        // destination's are, so they are carried as the one shape the window rule reads.
        hints: config.buffering_hints.as_ref().map(|hints| BufferingHints {
            size_in_m_bs: hints.size_in_m_bs,
            interval_in_seconds: hints.interval_in_seconds,
        }),
        retry_duration: config
            .retry_options
            .as_ref()
            .and_then(|options| options.duration_in_seconds)
            .and_then(|seconds| u64::try_from(seconds).ok())
            .map_or(DEFAULT_ENDPOINT_RETRY_DURATION, Duration::from_secs),
        backup: plain_target(&config.s3_configuration),
    }]
}

/// Every HTTP endpoint this stream's destinations deliver to right now.
fn stream_endpoints(stream: &DeliveryStream) -> Vec<HttpEndpointTarget> {
    stream
        .destinations
        .iter()
        .flat_map(|destination| http_endpoint_targets(&destination.config))
        .collect()
}

/// One Amazon OpenSearch Service domain index a stream's buffer is delivered to, with
/// everything the write needs, read off the destination configuration in force at the
/// moment of the flush.
///
/// The `Elasticsearch` and `Amazonopensearchservice` destination kinds are the same
/// offering under its old and its current name, so one target serves both.
#[derive(Debug, Clone)]
pub struct SearchTarget {
    /// `DomainARN` — "the ARN of the Amazon OpenSearch Service domain" — or, for a
    /// configuration naming only `ClusterEndpoint` ("specify either this ClusterEndpoint
    /// or the DomainARN field"), that endpoint's host. Either addresses the domain the
    /// documents are indexed into.
    pub domain: String,
    /// `IndexName`, before the rotation suffix [`rotated_index`] appends.
    pub index_name: String,
    /// An `IndexRotationPeriod` value, or `None` for the documented default of no
    /// rotation.
    pub index_rotation_period: Option<String>,
    /// `RoleARN` — "the IAM role to be assumed by Firehose … for indexing documents" —
    /// which is the principal the write is authorized as.
    pub role_arn: String,
    /// Whether Firehose names each document itself, which `DocumentIdOptions` decides.
    pub mints_document_id: bool,
    /// `BufferingHints`, which decide when this destination's buffer is due. The same two
    /// parameters as every other destination's, so one window rule serves them all
    /// ([`delivery_window`]).
    pub hints: Option<BufferingHints>,
    /// How long the delivery of one buffer is retried before its documents are parked in
    /// [`Self::backup`] — `RetryOptions$DurationInSeconds`, or
    /// [`DEFAULT_SEARCH_RETRY_DURATION`].
    pub retry_duration: Duration,
    /// The Amazon S3 location the destination's required `S3Configuration` names, where a
    /// document the domain did not index is written.
    pub backup: Option<S3Target>,
}

/// What the `firehose:error-output-type` namespace expands to for a document an Amazon
/// OpenSearch Service destination did not index, and the folder those documents are
/// written under when the backup configures no `ErrorOutputPrefix` of its own:
/// "Firehose writes any documents that could not be indexed to the configured Amazon S3
/// destination, with `AmazonOpenSearchService-failed/` appended to the key prefix"
/// (`S3BackupMode`).
const SEARCH_FAILED_OUTPUT_TYPE: &str = "AmazonOpenSearchService-failed";

/// `RetryOptions$DurationInSeconds` when the configuration names none: "The retry behavior
/// in case Firehose is unable to deliver documents to Amazon OpenSearch Service. The
/// default value is 300 (5 minutes)" (`AmazonopensearchserviceDestinationConfiguration$RetryOptions`).
const DEFAULT_SEARCH_RETRY_DURATION: Duration = Duration::from_secs(300);

/// The `DefaultDocumentIdFormat` value under which the delivery request carries NO document
/// id: "Firehose does not include any document IDs in the requests it sends to the Amazon
/// OpenSearch Service. This causes the Amazon OpenSearch Service domain to generate document
/// IDs" (`DocumentIdOptions$DefaultDocumentIdFormat`). Under the other value,
/// `FIREHOSE_DEFAULT`, "Firehose generates a unique document ID for each record based on a
/// unique internal identifier … stable across multiple delivery attempts", which is the
/// record's own id.
const NO_DOCUMENT_ID: &str = "NO_DOCUMENT_ID";

/// Every Amazon OpenSearch Service index a destination configuration indexes its documents
/// into — one for an `Elasticsearch` or `Amazonopensearchservice` destination, none for a
/// destination of any other kind.
///
/// The Serverless offering is NOT one of them: its documents go to a collection
/// (`AmazonOpenSearchServerlessDestinationConfiguration$CollectionEndpoint`), which is a
/// resource of the OpenSearch Serverless service and not a domain of this one, so this
/// emulator has no index to write them into.
#[must_use]
pub fn search_targets(config: &DestinationConfigKind) -> Vec<SearchTarget> {
    let target = match config {
        DestinationConfigKind::Elasticsearch(config) => search_target(
            config.domain_arn.as_deref(),
            config.cluster_endpoint.as_deref(),
            &config.index_name,
            config.index_rotation_period.as_deref(),
            &config.role_arn,
            config
                .document_id_options
                .as_ref()
                .map(|options| options.default_document_id_format.as_str()),
            config.buffering_hints.as_ref().map(|hints| BufferingHints {
                size_in_m_bs: hints.size_in_m_bs,
                interval_in_seconds: hints.interval_in_seconds,
            }),
            config
                .retry_options
                .as_ref()
                .and_then(|options| options.duration_in_seconds),
            &config.s3_configuration,
        ),
        DestinationConfigKind::AmazonOpenSearchService(config) => search_target(
            config.domain_arn.as_deref(),
            config.cluster_endpoint.as_deref(),
            &config.index_name,
            config.index_rotation_period.as_deref(),
            &config.role_arn,
            config
                .document_id_options
                .as_ref()
                .map(|options| options.default_document_id_format.as_str()),
            config.buffering_hints.as_ref().map(|hints| BufferingHints {
                size_in_m_bs: hints.size_in_m_bs,
                interval_in_seconds: hints.interval_in_seconds,
            }),
            config
                .retry_options
                .as_ref()
                .and_then(|options| options.duration_in_seconds),
            &config.s3_configuration,
        ),
        _ => None,
    };
    target.into_iter().collect()
}

/// The one search target a destination configuration of either Amazon OpenSearch Service
/// kind names, built from the members the two kinds carry alike.
///
/// `None` when the configuration addresses no domain at all (neither `DomainARN` nor
/// `ClusterEndpoint`) or names no index, in which case there is nothing to index into.
#[allow(clippy::too_many_arguments)]
fn search_target(
    domain_arn: Option<&str>,
    cluster_endpoint: Option<&str>,
    index_name: &str,
    index_rotation_period: Option<&str>,
    role_arn: &str,
    document_id_format: Option<&str>,
    hints: Option<BufferingHints>,
    retry_duration_in_seconds: Option<i32>,
    s3_configuration: &S3DestinationConfiguration,
) -> Option<SearchTarget> {
    let domain = domain_arn
        .map(str::to_string)
        .filter(|arn| !arn.is_empty())
        .or_else(|| {
            cluster_endpoint
                .and_then(authority_of)
                .filter(|host| !host.is_empty())
        })?;
    if index_name.is_empty() {
        return None;
    }
    Some(SearchTarget {
        domain,
        index_name: index_name.to_string(),
        index_rotation_period: index_rotation_period
            .filter(|period| !period.is_empty())
            .map(str::to_string),
        role_arn: role_arn.to_string(),
        mints_document_id: document_id_format != Some(NO_DOCUMENT_ID),
        hints,
        retry_duration: retry_duration_in_seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
            .map_or(DEFAULT_SEARCH_RETRY_DURATION, Duration::from_secs),
        backup: plain_target(s3_configuration),
    })
}

/// Every Amazon OpenSearch Service index this stream's destinations deliver to right now.
fn stream_searches(stream: &DeliveryStream) -> Vec<SearchTarget> {
    stream
        .destinations
        .iter()
        .flat_map(|destination| search_targets(&destination.config))
        .collect()
}

/// Every Splunk collector this stream's destinations deliver to right now.
fn stream_splunks(stream: &DeliveryStream) -> Vec<splunk::SplunkTarget> {
    stream
        .destinations
        .iter()
        .flat_map(|destination| splunk::splunk_targets(&destination.config))
        .collect()
}

/// When this stream's buffer comes due for delivery, or `None` when nothing is owed — an
/// empty buffer, or a destination with no location, no endpoint and no index to deliver
/// to.
#[must_use]
pub fn due_at(stream: &DeliveryStream) -> Option<SystemTime> {
    let opened_at = stream.records.first()?.received_at;
    // What the buffer HOLDS right now, which is what the size hint is about. Read off the
    // records themselves rather than off `buffered_bytes`, which also carries the bytes of
    // a delivery that is in flight and are no longer in this buffer (see [`take_due`]).
    let buffered: usize = stream.records.iter().map(|record| record.data.len()).sum();
    // The destinations are a few kinds with one buffering rule between them, and the FIRST
    // buffer to come due decides when the flush happens.
    let window = stream_targets(stream)
        .iter()
        .map(|target| delivery_window(target.hints.as_ref(), buffered))
        .chain(
            stream_endpoints(stream)
                .iter()
                .map(|endpoint| delivery_window(endpoint.hints.as_ref(), buffered)),
        )
        .chain(
            stream_searches(stream)
                .iter()
                .map(|search| delivery_window(search.hints.as_ref(), buffered)),
        )
        .chain(
            stream_splunks(stream)
                .iter()
                .map(|splunk| delivery_window(splunk.hints.as_ref(), buffered)),
        )
        .min()?;
    let due = opened_at + window;
    Some(match stream.delivery_retry_at {
        Some(retry_at) if retry_at > due => retry_at,
        _ => due,
    })
}

/// The earliest instant this store owes a delivery at, if any.
#[must_use]
pub fn next_due_at(store: &ComAmazonawsFirehoseFirehose20150804State) -> Option<SystemTime> {
    store.streams.values().filter_map(due_at).min()
}

/// Which deliveries a call to [`deliver_due`] performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extractions {
    /// Perform every due delivery, running a destination's `MetadataExtractionQuery` where
    /// one is configured. What the delivery pump does.
    Run,
    /// Perform only the deliveries that need no metadata-extraction program, leaving the
    /// rest to the pump. What a REQUEST does: a put must not be held while a program
    /// belonging to another stream's buffer runs.
    LeaveToThePump,
}

/// Take the buffer of every stream whose delivery `now` has already made due, at most
/// [`MAX_DELIVERIES_PER_CALL`] of them.
///
/// Writes nothing that was not already owed at `now`: a buffer still inside its window is
/// left exactly as it was, so a reporting operation calling this cannot change what a
/// later caller sees. What this leaves behind — because the cap was reached, or because
/// the caller leaves extraction-bearing deliveries to the pump — stays buffered, exactly
/// where it was, for a later delivery.
///
/// The records are taken out of the buffer but their bytes stay CHARGED against the buffer
/// budgets (`crate::state::MAX_BUFFERED_BYTES_PER_STREAM` and
/// `crate::state::MAX_BUFFERED_BYTES`) until the delivery is settled: they are still held
/// in this process, so a put that arrives while the delivery is in flight must not be able
/// to fill the whole budget again beside them. [`release_budget`] releases them once they
/// have been written; a delivery that could not be made is put back by [`restore`] with the
/// charge it never lost.
pub fn take_due(
    store: &mut ComAmazonawsFirehoseFirehose20150804State,
    now: SystemTime,
    extractions: Extractions,
) -> Vec<PendingDelivery> {
    let mut due = Vec::new();
    for stream in store.streams.values_mut() {
        if due.len() >= MAX_DELIVERIES_PER_CALL {
            break;
        }
        if due_at(stream).is_none_or(|due_at| due_at > now) {
            continue;
        }
        let targets = stream_targets(stream);
        let endpoints = stream_endpoints(stream);
        let searches = stream_searches(stream);
        let splunks = stream_splunks(stream);
        if targets.is_empty() && endpoints.is_empty() && searches.is_empty() && splunks.is_empty()
        {
            continue;
        }
        if extractions == Extractions::LeaveToThePump
            && targets
                .iter()
                .any(|target| target.metadata_extraction_query.is_some())
        {
            continue;
        }
        let records = ::std::mem::take(&mut stream.records);
        stream.delivery_retry_at = None;
        due.push(PendingDelivery {
            stream: stream.name.clone(),
            source_arn: stream.arn.clone(),
            version_id: stream.version_id,
            records,
            targets,
            endpoints,
            searches,
            splunks,
        });
    }
    due
}

/// Release the buffer budget the records of a DELIVERED buffer were charged against.
///
/// The counterpart of [`take_due`] keeping them charged: a byte a put accepted stays
/// charged from the moment it is accepted until it has been written to the destination, so
/// the bytes this process holds for one stream never exceed the one budget whatever a
/// producer does. A stream deleted while its delivery was in flight released its whole
/// buffer with it, so there is nothing left to release for it.
fn release_budget(
    store: &mut ComAmazonawsFirehoseFirehose20150804State,
    delivery: &PendingDelivery,
) {
    let bytes: usize = delivery
        .records
        .iter()
        .map(|record| record.data.len())
        .sum();
    let Some(stream) = store.streams.get_mut(&delivery.stream) else {
        return;
    };
    stream.buffered_bytes = stream.buffered_bytes.saturating_sub(bytes);
    store.buffered_bytes = store.buffered_bytes.saturating_sub(bytes);
}

/// Put a delivery that could not be made back in its stream's buffer, ahead of whatever
/// arrived since, and hold the next attempt off until [`DELIVERY_RETRY_SPAN`] has passed.
///
/// A record Firehose cannot deliver is not a record it drops: it keeps trying for the
/// documented retention. The stream being gone (deleted while the delivery was in flight)
/// takes its buffered data with it, so there is nothing to restore.
///
/// The budgets are not touched: these bytes stayed charged for the whole flight
/// ([`take_due`]), so putting the records back where they came from leaves the charge
/// exactly as the put that accepted them made it.
fn restore(
    store: &mut ComAmazonawsFirehoseFirehose20150804State,
    delivery: PendingDelivery,
    now: SystemTime,
) {
    let Some(stream) = store.streams.get_mut(&delivery.stream) else {
        return;
    };
    let mut records = delivery.records;
    records.append(&mut stream.records);
    stream.records = records;
    stream.delivery_retry_at = Some(now + DELIVERY_RETRY_SPAN);
}

/// Deliver every buffer this store owes at `now`.
///
/// The one entry point: a handler calls it (so a put also materialises what an earlier
/// buffer was owed), and so does the delivery pump. `extractions` is what separates the
/// two — see [`Extractions`]. `hop` is how a delivery reaches the other services it needs —
/// see [`DeliveryHop`].
pub async fn deliver_due<Eng: Send + Sync + 'static>(
    engine: &Eng,
    now: SystemTime,
    extractions: Extractions,
    hop: &DeliveryHop,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let pending = {
        let mut store = state.lock();
        store.materialise_due(now);
        take_due(&mut store, now, extractions)
    };
    for delivery in pending {
        // The HTTP endpoints and the Splunk collectors first, because a buffer one of them
        // has not taken and still has retries for is owed again AS A WHOLE: deciding that
        // before anything is written keeps the retry from writing the Amazon S3 half of the
        // same buffer twice.
        let mut parked = Vec::new();
        let mut owed = false;
        for endpoint in &delivery.endpoints {
            let Err(failure) = post_to_endpoint(hop.transport(), &delivery, endpoint, now).await
            else {
                continue;
            };
            if !retries_spent(&delivery, endpoint.hints.as_ref(), endpoint.retry_duration, now) {
                owed = true;
                continue;
            }
            // The retry window is spent, so these records are parked in the destination's
            // backup. One this emulator cannot work out — an `S3Configuration` naming no
            // bucket, a prefix that does not resolve — leaves the records owed instead of
            // dropping them.
            match endpoint
                .backup
                .as_ref()
                .and_then(|target| backup_object(&delivery, target, failure, now))
            {
                Some(object) => parked.push(object),
                None => owed = true,
            }
        }
        // Then the Splunk destinations, which are attempted the same way and whose events
        // the collector did not take are parked in the destination's Amazon S3 backup once
        // its retry window is spent: "Firehose writes any data that could not be indexed to
        // the configured Amazon S3 destination" (`SplunkS3BackupMode`), which holds under
        // both of that member's values.
        for collector in &delivery.splunks {
            let Err(failure) =
                splunk::post_to_collector(hop.transport(), &delivery, collector).await
            else {
                continue;
            };
            if !retries_spent(
                &delivery,
                collector.hints.as_ref(),
                collector.retry_duration,
                now,
            ) {
                owed = true;
                continue;
            }
            match collector
                .backup
                .as_ref()
                .and_then(|target| backup_object(&delivery, target, failure, now))
            {
                Some(object) => parked.push(object),
                None => owed = true,
            }
        }
        if owed {
            let mut store = state.lock();
            restore(&mut store, delivery, now);
            continue;
        }
        // Then the Amazon OpenSearch Service destinations: each record of the buffer is
        // one document, and the documents the domain did not take are parked in the
        // destination's Amazon S3 backup beside whatever the Amazon S3 locations get.
        for search in &delivery.searches {
            match index_documents(engine, &delivery, search, now).await {
                Delivered::Indexed(failures) if failures.is_empty() => {}
                Delivered::Indexed(failures) => {
                    match search_backup_object(&delivery, search, &failures, now) {
                        Some(object) => parked.push(object),
                        None => owed = true,
                    }
                }
                // The domain answered nothing — it is not provisioned, or no domain of
                // this emulator carries that address — so the whole buffer is owed again
                // for as long as the destination's retry window allows, and parked once
                // that window is spent. Firehose keeps a document it could not deliver
                // ("After this time has passed, the failed documents are written to
                // Amazon S3" — `RetryOptions$DurationInSeconds`).
                Delivered::Unavailable(failure) => {
                    if retries_spent(&delivery, search.hints.as_ref(), search.retry_duration, now) {
                        let failures: Vec<SearchDocumentFailure> = delivery
                            .records
                            .iter()
                            .map(|record| SearchDocumentFailure {
                                record_id: record.record_id.clone(),
                                index_name: failure.index_name.clone(),
                                document_id: document_id(search, record),
                                error_code: failure.error_code.clone(),
                                error_message: failure.error_message.clone(),
                            })
                            .collect();
                        match search_backup_object(&delivery, search, &failures, now) {
                            Some(object) => parked.push(object),
                            None => owed = true,
                        }
                    } else {
                        owed = true;
                    }
                }
            }
        }
        if owed {
            let mut store = state.lock();
            restore(&mut store, delivery, now);
            continue;
        }
        // A delivery this emulator could not WORK OUT right now — an encoder that refused
        // the body — is owed again, exactly like one the destination could not take:
        // nothing of it is written and nothing of it is lost.
        let Some(mut objects) = objects_of(engine, &delivery, now).await else {
            let mut store = state.lock();
            restore(&mut store, delivery, now);
            continue;
        };
        objects.append(&mut parked);
        // The write is the destination ROLE's to make, so what that role is allowed to do
        // decides whether it is made at all: a delivery the role may not write is not
        // written, is reported through the destination's own logging and is owed again, which
        // is what a producer observes of it (nothing but the error event).
        let region = delivery_region(&delivery.source_arn).unwrap_or_default();
        let (withheld, mut reported) = withheld_writes(hop, region, &objects).await;
        if withheld {
            destination_logging::report_delivery_errors(
                engine,
                region,
                &delivery.source_arn,
                delivery.version_id,
                &reported,
                now,
            );
            let mut store = state.lock();
            restore(&mut store, delivery, now);
            continue;
        }
        let written = write_objects(engine, objects);
        reported.extend(written.reported);
        // A destination that did not take this delivery reports it, whether the rest of the
        // delivery was written or not: the records of the part that was not written are owed
        // again below, and the error event is the only thing a producer can observe of them.
        destination_logging::report_delivery_errors(
            engine,
            region,
            &delivery.source_arn,
            delivery.version_id,
            &reported,
            now,
        );
        if written.any {
            // Written, so these records are no longer held anywhere in this process and
            // the budget they were charged against is theirs no longer.
            let mut store = state.lock();
            release_budget(&mut store, &delivery);
            continue;
        }
        // The Amazon S3 half of this buffer could not be written, so the whole of it is
        // owed again — including a POST an endpoint already acknowledged, which a
        // destination may therefore see twice. Firehose's own delivery is at-least-once
        // ("Re-invoking the Put API operations […] can result in data duplicates" —
        // `PutRecord`), and this is the answer that loses nothing.
        let mut store = state.lock();
        restore(&mut store, delivery, now);
    }
}

/// POST one buffer to an HTTP endpoint destination, answering how the delivery failed when
/// the endpoint did not acknowledge it.
///
/// The request is the one the delivery protocol `HttpEndpointConfiguration$Url` points at
/// describes: a `POST` of the JSON envelope [`request_envelope`] renders, with the protocol
/// version, the request id the envelope also carries, the Firehose stream the records came
/// from, the configured access key and common attributes, and the body gzipped where
/// `ContentEncoding` asks for it. The acknowledgment is a success status whose answer
/// ACKNOWLEDGES this request ([`acknowledges`]); any other answer, and any request that
/// could not be made at all, is a failed delivery.
///
/// The request goes through the IN-PROCESS emulator transport and NOTHING else, so a URL a
/// caller configured reaches only what this emulator serves itself: no socket is opened
/// and no name is resolved.
async fn post_to_endpoint(
    transport: &SharedHttpConnector,
    delivery: &PendingDelivery,
    endpoint: &HttpEndpointTarget,
    now: SystemTime,
) -> Result<(), DeliveryFailure> {
    let request_id = ::uuid::Uuid::new_v4().to_string();
    // The body is this buffer's records, so its size is bounded by the per-stream buffer
    // budget (`crate::state::MAX_BUFFERED_BYTES_PER_STREAM`) the put already charged
    // against, plus the base64 expansion of it.
    let body = ::serde_json::to_vec(&request_envelope(&request_id, delivery, now))
        .map_err(|_| HTTP_ENDPOINT_CONNECTION_FAILED)?;
    let body = if endpoint.gzip {
        gzip(&body).ok_or(HTTP_ENDPOINT_CONNECTION_FAILED)?
    } else {
        body
    };
    let mut request = ::http::Request::builder()
        .method(::http::Method::POST)
        .uri(&endpoint.url)
        .header(::http::header::CONTENT_TYPE, "application/json")
        .header(PROTOCOL_VERSION_HEADER, PROTOCOL_VERSION)
        .header(REQUEST_ID_HEADER, &request_id)
        .header(SOURCE_ARN_HEADER, &delivery.source_arn);
    if let Some(host) = authority_of(&endpoint.url) {
        request = request.header(::http::header::HOST, host);
    }
    if endpoint.gzip {
        request = request.header(::http::header::CONTENT_ENCODING, GZIP_CONTENT_ENCODING_TOKEN);
    }
    if let Some(access_key) = endpoint.access_key.as_deref() {
        request = request.header(ACCESS_KEY_HEADER, access_key);
    }
    if !endpoint.common_attributes.is_empty() {
        request = request.header(
            COMMON_ATTRIBUTES_HEADER,
            common_attributes_header(&endpoint.common_attributes),
        );
    }
    // Every value above comes from the configuration, so a `Url` that is not a URI or an
    // attribute that is not a header value fails HERE rather than panicking.
    let request = request
        .body(::aws_smithy_types::body::SdkBody::from(
            ::bytes::Bytes::from(body),
        ))
        .map_err(|error| error.to_string())
        .and_then(|request| {
            ::aws_smithy_runtime_api::http::Request::try_from(request)
                .map_err(|error| error.to_string())
        });
    let request = match request {
        Ok(request) => request,
        Err(error) => {
            ::tracing::warn!(
                stream = %delivery.stream,
                error = %error,
                "the HTTP endpoint destination's configuration does not render a delivery \
                 request, so nothing was delivered to it"
            );
            return Err(HTTP_ENDPOINT_CONNECTION_FAILED);
        }
    };
    let response = HttpConnector::call(transport, request)
        .await
        .map_err(|error| {
            ::tracing::warn!(
                stream = %delivery.stream,
                error = %error,
                "the HTTP endpoint destination could not be reached, so this delivery was \
                 left for a later one"
            );
            HTTP_ENDPOINT_CONNECTION_FAILED
        })?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        ::tracing::warn!(
            stream = %delivery.stream,
            status,
            "the HTTP endpoint destination did not acknowledge this delivery"
        );
        return Err(HTTP_ENDPOINT_DESTINATION_EXCEPTION);
    }
    if let Err(reason) = acknowledges(&response, &request_id) {
        ::tracing::warn!(
            stream = %delivery.stream,
            status,
            reason,
            "the HTTP endpoint destination's answer does not acknowledge this delivery"
        );
        return Err(HTTP_ENDPOINT_INVALID_RESPONSE);
    }
    Ok(())
}

/// Whether the endpoint's answer ACKNOWLEDGES the delivery request `request_id`, or why it
/// does not.
///
/// A success status is not by itself the acknowledgment: the delivery protocol
/// `HttpEndpointConfiguration$Url` points at asks a destination to answer one JSON document
/// naming the request it is answering, and to report a request it could not process in that
/// document's own error member rather than in the status line. So an answer hands this
/// buffer over only when its body is a JSON object whose `requestId` is THIS request's and
/// whose error member is absent or empty; anything else — an unreadable or non-JSON body, a
/// body naming no request or another one, a body reporting an error — is a buffer the
/// destination has not taken, which is then retried and finally parked exactly like one it
/// refused outright.
fn acknowledges(
    response: &::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    request_id: &str,
) -> Result<(), &'static str> {
    let Some(body) = response.body().bytes() else {
        return Err("the answer's body could not be read");
    };
    let Ok(answer) = ::serde_json::from_slice::<::serde_json::Value>(body) else {
        return Err("the answer's body is not JSON");
    };
    let Some(answered) = answer
        .get(ANSWER_REQUEST_ID_MEMBER)
        .and_then(::serde_json::Value::as_str)
    else {
        return Err("the answer names no request");
    };
    if answered != request_id {
        return Err("the answer names another request");
    }
    match answer.get(ANSWER_ERROR_MESSAGE_MEMBER) {
        None | Some(::serde_json::Value::Null) => Ok(()),
        Some(::serde_json::Value::String(message)) if message.is_empty() => Ok(()),
        Some(_) => Err("the answer reports an error of its own"),
    }
}

/// The member of the endpoint's answer naming the delivery request it answers, which an
/// acknowledgment names this request in.
const ANSWER_REQUEST_ID_MEMBER: &str = "requestId";

/// The member of the endpoint's answer reporting that the destination did not process the
/// delivered buffer, so an answer carrying one is not an acknowledgment whatever its status
/// said.
const ANSWER_ERROR_MESSAGE_MEMBER: &str = "errorMessage";

/// The JSON body of one delivery request: the request's own id, the instant it was made and
/// one entry per buffered record carrying that record's data base64-encoded, in arrival
/// order.
fn request_envelope(
    request_id: &str,
    delivery: &PendingDelivery,
    now: SystemTime,
) -> ::serde_json::Value {
    let records: Vec<::serde_json::Value> = delivery
        .records
        .iter()
        .map(|record| {
            ::serde_json::json!({ "data": ::aws_smithy_types::base64::encode(&record.data) })
        })
        .collect();
    ::serde_json::json!({
        "requestId": request_id,
        "timestamp": epoch_millis(now),
        "records": records,
    })
}

/// `RequestConfiguration$CommonAttributes` as the one header value that carries them: a
/// JSON object of name to value, under the `commonAttributes` member the delivery protocol
/// names. Configuration order is kept, and a name configured twice keeps the last value —
/// one header member can carry one value for a name.
fn common_attributes_header(attributes: &[(String, String)]) -> String {
    let mut common = ::serde_json::Map::new();
    for (name, value) in attributes {
        common.insert(name.clone(), ::serde_json::Value::from(value.as_str()));
    }
    ::serde_json::json!({ "commonAttributes": common }).to_string()
}

/// The authority of `url`, which the request names as its `Host`, or `None` when the value
/// carries none.
fn authority_of(url: &str) -> Option<String> {
    let uri: ::http::Uri = url.parse().ok()?;
    uri.authority().map(::std::string::ToString::to_string)
}

/// Whether the retry window a destination allows has been spent, so this buffer's records
/// are parked in the destination's backup rather than attempted again.
///
/// "The total amount of time that Firehose spends on retries. This duration starts after
/// the initial attempt to send data to the custom destination via HTTPS endpoint fails"
/// (`HttpEndpointRetryOptions$DurationInSeconds`) — and the initial attempt is the one made
/// when the buffer came due, so the window closes that long after it. A configured ZERO is
/// a destination that is not retried at all, which its own range (`min: 0`) admits. The
/// rule is the destination's `RetryOptions` whichever kind carries it, so the window is
/// computed from the two values every kind states: its buffering hints and its retry
/// duration.
fn retries_spent(
    delivery: &PendingDelivery,
    hints: Option<&BufferingHints>,
    retry_duration: Duration,
    now: SystemTime,
) -> bool {
    let Some(opened_at) = delivery.records.first().map(|record| record.received_at) else {
        return true;
    };
    let buffered: usize = delivery.records.iter().map(|record| record.data.len()).sum();
    let first_attempt = opened_at + delivery_window(hints, buffered);
    now >= first_attempt + retry_duration
}

/// The object that parks the records a destination never took in its Amazon S3 backup:
/// "only the documents that Firehose could not deliver to the specified HTTP endpoint
/// destination (`FailedDataOnly`)" (`S3BackupMode`), and the same for every other
/// destination whose backup takes the records its primary destination rejected.
///
/// Written under the backup's `ErrorOutputPrefix` as the error-output document of each
/// record ([`error_envelope`]), so a consumer of that prefix learns how the delivery failed
/// and still holds the record's own bytes. `None` when the backup cannot be worked out at
/// all, in which case the records stay owed rather than being dropped.
fn backup_object(
    delivery: &PendingDelivery,
    target: &S3Target,
    failure: DeliveryFailure,
    now: SystemTime,
) -> Option<DeliveredObject> {
    let prefix = error_output_prefix(target, failure, now).ok()?;
    let records: Vec<&StoredRecord> = delivery.records.iter().collect();
    let encoded = compressed(
        error_payload(&records, failure, now, None, &BTreeMap::new()),
        target,
    )?;
    Some(DeliveredObject {
        bucket: target.bucket.clone(),
        key: format!(
            "{prefix}{}{}",
            object_name(delivery, now, target.time_zone),
            encoded.extension
        ),
        body: encoded.body,
        content_encoding: encoded.content_encoding,
        kms_key_arn: target.kms_key_arn.clone(),
        destination: target.destination(),
    })
}

/// The `errorCode` the error output names for a buffer whose destination domain answered
/// NOTHING — it is not provisioned, or this emulator holds no domain at that address — so
/// no cluster ever judged the documents. Written in the same form as the other
/// destinations' codes here (`<destination>.<failure>`), because it names the destination
/// rather than a document's own refusal, whose code is the engine's own error type.
const SEARCH_DESTINATION_UNAVAILABLE: &str = "AmazonOpenSearchService.ServiceUnavailable";

/// How ONE document of a buffer failed to be indexed, as the error-output document names
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchDocumentFailure {
    /// The record the document was made from, which names the record in the buffer this
    /// failure belongs to.
    record_id: String,
    /// The index the document was written to, rotation suffix and all.
    index_name: String,
    /// The id the document was written under, empty where the delivery named none.
    document_id: String,
    /// The engine's own error type, which is what a consumer of the error output keys
    /// off.
    error_code: String,
    /// The engine's reason for refusing the document.
    error_message: String,
}

/// What a delivery to one Amazon OpenSearch Service destination did.
#[derive(Debug, Clone)]
enum Delivered {
    /// The domain answered: every record was offered to it, and these are the documents
    /// it did not take (empty when it took them all).
    Indexed(Vec<SearchDocumentFailure>),
    /// The domain answered NOTHING — it is not provisioned, or this emulator holds no
    /// domain at that address — so the buffer has not been delivered at all.
    Unavailable(SearchDocumentFailure),
}

/// Index every record of `delivery` into `search`'s index as one document.
///
/// Each record is one document: "Firehose buffers records before delivering them to the
/// destination" (`PutRecord`), and for a destination in Amazon OpenSearch Service the
/// delivery is the indexing of those records. The write goes through the OpenSearch
/// Service's OWN store ([`::service_com_amazonaws_opensearch_amazonopensearchservice::state::ingest`]),
/// which is the one place that decides whether the destination's role may write, which
/// index the document lands in and whether that index's mappings accept it — so a document
/// this emulator indexes is readable from the domain that holds it, and a document the
/// domain refuses comes back with the engine's own error type for the backup to carry.
///
/// A document is offered to the domain ONCE per flush, and a buffer whose backup could not
/// be written is owed again as a whole: a document that was already indexed is then
/// written again under the SAME id ([`document_id`]), which is why Firehose's own id is
/// "stable across multiple delivery attempts" — the second write replaces the first rather
/// than adding a copy.
///
/// ASYNC because a buffer holds as many records as its byte budget allows and each one is a
/// write: the loop hands the runtime back between documents, so a timeout or a cancellation
/// can land while a long buffer is being indexed instead of the whole run holding the
/// worker. The domain's own store is locked by each write and never across one of those
/// yields.
async fn index_documents<Eng: Send + Sync + 'static>(
    engine: &Eng,
    delivery: &PendingDelivery,
    search: &SearchTarget,
    now: SystemTime,
) -> Delivered {
    use ::service_com_amazonaws_opensearch_amazonopensearchservice::state::ingest;

    let index_name = rotated_index(search, now);
    let mut failures = Vec::new();
    for record in &delivery.records {
        ::tokio::task::yield_now().await;
        let document_id = document_id(search, record);
        let write = ingest::DocumentWrite {
            domain: &search.domain,
            index_name: &index_name,
            document_id: (!document_id.is_empty()).then_some(document_id.as_str()),
            document: &record.data,
            principal_arn: &search.role_arn,
            now,
        };
        match ingest::index_document(engine, &write) {
            Ok(_) => {}
            Err(ingest::WriteRefusal::Rejected {
                error_type,
                message,
            }) => failures.push(SearchDocumentFailure {
                record_id: record.record_id.clone(),
                index_name: index_name.clone(),
                document_id,
                error_code: error_type.to_string(),
                error_message: message,
            }),
            // The domain answered nothing, which is a verdict on the DESTINATION and not
            // on this document: the whole buffer is owed again.
            Err(ingest::WriteRefusal::Unavailable { message }) => {
                ::tracing::warn!(
                    stream = %delivery.stream,
                    error = %message,
                    "the Amazon OpenSearch Service destination could not be reached, so this \
                     delivery was left for a later one"
                );
                return Delivered::Unavailable(SearchDocumentFailure {
                    record_id: record.record_id.clone(),
                    index_name,
                    document_id,
                    error_code: SEARCH_DESTINATION_UNAVAILABLE.to_string(),
                    error_message: message,
                });
            }
        }
    }
    Delivered::Indexed(failures)
}

/// The id one record's document is written under: the record's own id where Firehose names
/// the document itself, and none where `DocumentIdOptions` leaves the naming to the domain.
///
/// "Firehose generates a unique document ID for each record based on a unique internal
/// identifier. The generated document ID is stable across multiple delivery attempts, which
/// helps prevent the same record from being indexed multiple times with different document
/// IDs" (`DocumentIdOptions$DefaultDocumentIdFormat`) — and the record's `RecordId`, which
/// the put answered with, is exactly such an identifier.
fn document_id(search: &SearchTarget, record: &StoredRecord) -> String {
    if search.mints_document_id {
        record.record_id.clone()
    } else {
        String::new()
    }
}

/// The index one document of this flush is written into: `IndexName` with the rotation
/// suffix its `IndexRotationPeriod` appends.
///
/// "The Amazon OpenSearch Service index rotation period. Index rotation appends a timestamp
/// to the IndexName to facilitate the expiration of old data" (`IndexRotationPeriod`), and
/// the timestamp is the period the rotation is over: the hour, the day, the ISO week or the
/// month of the delivery, each as the UTC calendar names it. `NoRotation` — the value a
/// configuration naming none gets — appends nothing, and so does an instant the calendar
/// cannot place, which leaves the delivery writing the index the configuration named rather
/// than one named for nothing. The calendar fields come from the maintained date library
/// rather than from arithmetic here.
fn rotated_index(search: &SearchTarget, now: SystemTime) -> String {
    let Some(period) = search.index_rotation_period.as_deref() else {
        return search.index_name.clone();
    };
    // The instant as the calendar's own type, fallibly: `SystemTime` reaches further than
    // the calendar does, so an instant outside it is answered with no suffix instead of
    // panicking inside the conversion.
    let at = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|since_epoch| i64::try_from(since_epoch.as_secs()).ok())
        .and_then(|seconds| ::chrono::DateTime::from_timestamp(seconds, 0));
    let stamped = |pattern: &str| {
        at.map(|at| at.format(pattern).to_string())
            .unwrap_or_default()
    };
    let suffix = match period {
        "OneHour" => stamped("-%Y-%m-%d-%H"),
        "OneDay" => stamped("-%Y-%m-%d"),
        // The ISO week, with the week-based year it belongs to: `%G-w%V`, the pair that
        // keeps a week spanning a year boundary in one index.
        "OneWeek" => stamped("-%G-w%V"),
        "OneMonth" => stamped("-%Y-%m"),
        // `NoRotation`, and any value this emulator does not know: the index is the one
        // the configuration named.
        _ => String::new(),
    };
    format!("{}{suffix}", search.index_name)
}

/// The object that parks the documents the domain did not index in the destination's
/// Amazon S3 backup: "Firehose writes any documents that could not be indexed to the
/// configured Amazon S3 destination" (`S3BackupMode`), which it does under both of that
/// member's modes.
///
/// Written as the error-output document of each failed document ([`search_error_envelope`]),
/// so a consumer of the backup learns which index refused it and why, and still holds the
/// record's own bytes. `None` when the backup cannot be worked out at all, in which case
/// the records stay owed rather than being dropped.
///
/// The buffer's records are indexed by their `RecordId` ONCE, so resolving every failure
/// back to the record it is about is one pass over the failures and not one pass over the
/// buffer per failure: a domain that is not reachable fails every record of the buffer at
/// once, and the records a buffer may hold are bounded by a byte budget rather than a count.
fn search_backup_object(
    delivery: &PendingDelivery,
    search: &SearchTarget,
    failures: &[SearchDocumentFailure],
    now: SystemTime,
) -> Option<DeliveredObject> {
    let target = search.backup.as_ref()?;
    let prefix = search_error_prefix(target, now).ok()?;
    let records: ::std::collections::HashMap<&str, &StoredRecord> = delivery
        .records
        .iter()
        .map(|record| (record.record_id.as_str(), record))
        .collect();
    let documents: Vec<String> = failures
        .iter()
        .map(|failure| {
            let record = records.get(failure.record_id.as_str()).copied();
            search_error_envelope(record, failure, now).to_string()
        })
        .collect();
    let encoded = compressed(documents.join("\n").into_bytes(), target)?;
    Some(DeliveredObject {
        bucket: target.bucket.clone(),
        key: format!(
            "{prefix}{}{}",
            object_name(delivery, now, target.time_zone),
            encoded.extension
        ),
        body: encoded.body,
        content_encoding: encoded.content_encoding,
        kms_key_arn: target.kms_key_arn.clone(),
        destination: target.destination(),
    })
}

/// The prefix a document the domain did not index is written under.
///
/// `ErrorOutputPrefix` where the backup configures one — "a prefix that Firehose evaluates
/// and adds to failed records before writing them to S3" — and otherwise the backup's own
/// `Prefix` with [`SEARCH_FAILED_OUTPUT_TYPE`] appended, which is what the mode documents:
/// "with `AmazonOpenSearchService-failed/` appended to the key prefix" (`S3BackupMode`).
fn search_error_prefix(target: &S3Target, now: SystemTime) -> Result<String, DeliveryFailure> {
    let configured = if target.error_output_prefix.is_empty() {
        format!("{}{SEARCH_FAILED_OUTPUT_TYPE}/", target.prefix)
    } else {
        target.error_output_prefix.clone()
    };
    expand_prefix(
        &configured,
        &BTreeMap::new(),
        Some(SEARCH_FAILED_OUTPUT_TYPE),
        now,
        target.time_zone,
    )
}

/// The error-output document of one document the domain did not index: how the indexing
/// failed, when it was attempted, which index and document id it was attempted under, and
/// the record's own data.
///
/// The common members are the ones every Firehose error output carries (`errorCode`,
/// `errorMessage`, `attemptsMade`, `arrivalTimestamp`, `attemptEndingTimestamp`,
/// `rawData`), and the three `es…` members are the ones the error output of an Amazon
/// OpenSearch Service destination adds: the index and the document id the delivery used.
/// `errorCode` is the engine's own error type, which is what the domain's answer carried.
fn search_error_envelope(
    record: Option<&StoredRecord>,
    failure: &SearchDocumentFailure,
    now: SystemTime,
) -> ::serde_json::Value {
    ::serde_json::json!({
        "attemptsMade": 1,
        "arrivalTimestamp": record.map_or_else(|| epoch_millis(now), |record| epoch_millis(record.received_at)),
        "errorCode": failure.error_code,
        "errorMessage": failure.error_message,
        "attemptEndingTimestamp": epoch_millis(now),
        "rawData": record.map(|record| ::aws_smithy_types::base64::encode(&record.data)).unwrap_or_default(),
        "esDocumentId": failure.document_id,
        "esIndexName": failure.index_name,
    })
}

/// The `ServerSideEncryption` value an object encrypted with a KMS key is stored under:
/// the single-layer one of the two values Amazon S3 publishes for KMS keys, read from that
/// service's own set so the two can never drift.
const SSE_KMS_ALGORITHM: &str = ::service_com_amazonaws_s3_amazons3::state::KMS_SSE_ALGORITHMS[0];

/// How a delivered object is encrypted at rest.
///
/// A destination whose `EncryptionConfiguration` names an encryption key
/// ([`S3Target::kms_key_arn`]) has its objects written under SSE-KMS with that key, so a
/// later read of one reports the algorithm and the key it was stored with. A destination
/// naming no key writes an object that asks for no encryption of its own, which is the case
/// where the bucket's own default encryption configuration decides what it is stored under
/// — the same rule every other write to that bucket follows.
fn object_encryption(
    store: &::service_com_amazonaws_s3_amazons3::state::ComAmazonawsS3Amazons3State,
    object: &DeliveredObject,
) -> ::service_com_amazonaws_s3_amazons3::state::ObjectEncryption {
    match &object.kms_key_arn {
        Some(key) => ::service_com_amazonaws_s3_amazons3::state::ObjectEncryption {
            sse_algorithm: Some(SSE_KMS_ALGORITHM.to_string()),
            kms_key_id: Some(key.clone()),
            kms_encryption_context: None,
            bucket_key_enabled: None,
            // A delivery supplies no customer-provided key: the destination configuration
            // names a KMS key or nothing at all.
            ..Default::default()
        },
        None => store
            .bucket(&object.bucket)
            .map(|bucket| bucket.default_object_encryption())
            .unwrap_or_default(),
    }
}

/// What the write of one delivery's objects did.
#[derive(Debug, Default)]
struct Written {
    /// Whether any object reached the destination. `false` means the delivery is owed again
    /// as a whole; `true` with failures beside it means part of it was written, which is
    /// reported rather than retried, because retrying would deliver the written object twice.
    any: bool,
    /// The failures the destinations' own `CloudWatchLoggingOptions` report
    /// ([`destination_logging`]).
    reported: Vec<DeliveryErrorReport>,
}

/// Write `objects` into the one object store, answering what reached the destination and
/// what each destination's logging has to report.
///
/// A bucket a `BucketARN` names that was never created takes nothing, and neither does a
/// store this engine does not hold: both are deliveries the destination did not take, so the
/// records stay buffered and the destination's logging reports the condition.
fn write_objects<Eng: Send + Sync + 'static>(
    engine: &Eng,
    objects: Vec<DeliveredObject>,
) -> Written {
    if objects.is_empty() {
        return Written {
            any: true,
            reported: Vec::new(),
        };
    }
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        ::tracing::warn!("the object store is not reachable from this engine, so nothing was delivered");
        return Written::default();
    };
    let mut store = state.lock();
    let mut written = 0usize;
    let mut reported = Vec::new();
    for object in objects {
        // The bucket the destination names does not exist, so there is nothing to write
        // into: the condition Amazon S3 answers such a write with, reported through the
        // destination's own logging.
        if store.bucket(&object.bucket).is_none() {
            ::tracing::warn!(
                bucket = %object.bucket,
                "the destination bucket does not exist, so this delivery was not written"
            );
            report(&mut reported, &object.destination, S3_NO_SUCH_BUCKET);
            continue;
        }
        let encryption = object_encryption(&store, &object);
        let stored = ::service_com_amazonaws_s3_amazons3::state::Object {
            // The entity tag is computed from the bytes the object is stored with,
            // exactly as it is for an object written through `PutObject`.
            etag: ::service_com_amazonaws_s3_amazons3::state::object_etag(&object.body),
            body: object.body,
            version_id: None,
            last_modified: SystemTime::now(),
            storage_class: ::service_com_amazonaws_s3_amazons3::state::DEFAULT_STORAGE_CLASS
                .to_string(),
            content_type: Some(DELIVERED_CONTENT_TYPE.to_string()),
            cache_control: None,
            content_disposition: None,
            // What the body was encoded with, so a reader of the object is told which
            // decoder its bytes are for ([`compressed`]).
            content_encoding: object.content_encoding,
            content_language: None,
            expires: None,
            website_redirect_location: None,
            metadata: BTreeMap::new(),
            tags: Vec::new(),
            encryption,
            // A delivered object carries no Object Lock retention: a retention is placed
            // on a stored version afterwards, by S3's own `PutObjectRetention`.
            retention: None,
            // Nor a legal hold, for the same reason: S3's own `PutObjectLegalHold` is what
            // applies one to a stored version.
            legal_hold: None,
            // And no restore: a delivered object is stored in a non-archive storage class,
            // so there is nothing to restore; S3's own `RestoreObject` is what asks for one.
            restore: None,
            // And no annotation: one is attached to a stored object afterwards, by S3's own
            // `PutObjectAnnotation`.
            annotations: BTreeMap::new(),
            // And no ACL: a delivered object carries the ACL every object starts with, the
            // owner's `FULL_CONTROL`, which a reader derives; S3's own `PutObjectAcl` is
            // what sets one on it.
            acl: None,
            // And no checksum: a delivery names no checksum algorithm for the object to be
            // stored under, so a read of it reports none, exactly as for an object written
            // through `PutObject` without one.
            checksum: None,
            // Nor is it assembled from parts: a delivered object is written whole.
            parts: Vec::new(),
            // And nothing replicated it: S3's own store settles a replication record from
            // the bucket's replication configuration when it records a write through its
            // own data plane.
            replication: None,
        };
        match store.put_object(&object.bucket, &object.key, stored) {
            Some(
                ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Created
                | ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Replaced,
            ) => {
                written += 1;
            }
            outcome => ::tracing::warn!(
                bucket = %object.bucket,
                key = %object.key,
                ?outcome,
                "the destination bucket did not take a delivered object"
            ),
        }
    }
    Written {
        any: written > 0,
        reported,
    }
}

/// Note one failed delivery for the destination's logging to report, once per
/// (destination, condition): a delivery writing several objects to one destination failed
/// ONCE, so it is reported once however many of its objects the destination did not take.
///
/// A destination whose `CloudWatchLoggingOptions` report nothing has nothing noted here; the
/// records are owed again either way, which is what the caller decides from.
fn report(
    reports: &mut Vec<DeliveryErrorReport>,
    destination: &ObjectDestination,
    error: DestinationError,
) {
    let Some(logging) = destination.logging.clone() else {
        return;
    };
    let report = DeliveryErrorReport {
        logging,
        destination: destination.bucket_arn.clone(),
        error,
    };
    if !reports.contains(&report) {
        reports.push(report);
    }
}

/// The objects of this delivery the destination's ROLE may not write, as the failures its
/// logging reports — paired with whether any object was withheld at all.
///
/// Firehose writes a delivered object under the role the destination configuration names
/// ("the Amazon Resource Name (ARN) of the Amazon Web Services credentials" —
/// `S3DestinationConfiguration$RoleARN`), so the write is that role's to make: it happens
/// only where the permissions in force on the role allow Amazon S3's own write of the object
/// it would create. That decision is IAM's, asked of IAM
/// ([`service_common::iam_role::role_withholds_action_through`]), so the documents in force
/// are collected and decided by the service that holds them.
///
/// Nothing is withheld where there is no one to ask (a hop built over a transport alone), nor
/// for a destination naming no role at all, nor for anything that is not IAM ANSWERING that
/// the role does not allow the write — a role this account's IAM holds nothing under
/// included, exactly as that question is read everywhere else in this emulator.
async fn withheld_writes(
    hop: &DeliveryHop,
    region: &str,
    objects: &[DeliveredObject],
) -> (bool, Vec<DeliveryErrorReport>) {
    let Some(emulator) = hop.emulator() else {
        return (false, Vec::new());
    };
    let mut withheld = false;
    let mut reported = Vec::new();
    for object in objects {
        let destination = &object.destination;
        if destination.role_arn.is_empty() {
            continue;
        }
        // The resource is the object this write would create, which is what an
        // object-granting statement (`arn:<partition>:s3:::<bucket>/<key pattern>`) is
        // written about.
        let resource = format!("{}/{}", destination.bucket_arn, object.key);
        if !::service_common::iam_role::role_withholds_action_through(
            emulator,
            region,
            &destination.role_arn,
            S3_PUT_OBJECT_ACTION,
            &resource,
        )
        .await
        {
            continue;
        }
        ::tracing::warn!(
            bucket = %object.bucket,
            key = %object.key,
            "the destination role is not allowed to write this object, so this delivery was \
             not written"
        );
        withheld = true;
        report(&mut reported, destination, S3_ACCESS_DENIED);
    }
    (withheld, reported)
}

/// The Region a delivery of `stream_arn` asks its authorization question in: the Region that
/// stream lives in, read off its own ARN.
///
/// A delivery is made by no request of its own, so there is no credential scope to read it
/// from; the stream's ARN was minted in the Region the request that created the stream was
/// signed for, which is the Region its roles and destinations are resolved in. A value that
/// is not a Firehose stream ARN names no Region, which leaves the question unanswerable and
/// so withholds nothing.
fn delivery_region(stream_arn: &str) -> Option<&str> {
    ::service_common::arn::region_of(stream_arn, FIREHOSE_ARN_VENDOR)
}

/// The objects one delivery writes: one per destination location and, where dynamic
/// partitioning is enabled, one per distinct set of partition keys plus one per reason a
/// record could not be placed.
///
/// `None` when the delivery could not be worked out at all right now and is owed again
/// (see [`grouped`]), so the caller puts it back rather than writing part of it.
///
/// `engine` is the one shared engine every mounted service reaches its state through: a
/// location whose destination converts its data format reads the bound Glue table's columns
/// out of the Data Catalog through it ([`format_conversion`]). This service's own store is
/// NOT locked here, so the two are never held at once.
async fn objects_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    delivery: &PendingDelivery,
    now: SystemTime,
) -> Option<Vec<DeliveredObject>> {
    let mut objects = Vec::new();
    // What the records of THIS delivery may inflate to, over every location they go to:
    // charged down as they are processed, so one flush cannot hold more than the budget
    // whatever its records' compression ratio is.
    let mut inflation_budget =
        crate::state::cloudwatch_logs_pipeline::MAX_INFLATED_BYTES_PER_DELIVERY;
    for target in &delivery.targets {
        // The prefix each object of this location is written under, with the records that
        // go in it. A group whose own prefix cannot be resolved — the prefix names a
        // partition key the query derived no value for — joins the failed records, which
        // is what the error output prefix is for: it is never dropped.
        let mut writing: Vec<(String, Vec<&StoredRecord>)> = Vec::new();
        let mut failed: BTreeMap<DeliveryFailure, Vec<&StoredRecord>> = BTreeMap::new();
        for group in grouped(target, &delivery.records).await? {
            match group.keys {
                GroupKeys::Derived(keys) => {
                    match expand_prefix(&target.prefix, &keys, None, now, target.time_zone) {
                        Ok(prefix) => writing.push((prefix, group.records)),
                        Err(failure) => failed.entry(failure).or_default().extend(group.records),
                    }
                }
                GroupKeys::Failed(failure) => {
                    failed.entry(failure).or_default().extend(group.records);
                }
            }
        }
        // Each object of this location, with the bytes it carries: a delivered one holds
        // what the destination's processing left of its records' data — in the format its
        // conversion configuration asks for, where one is in force — and an error-output
        // one holds the error document of each record that could not be placed, including
        // one the processing itself could not read.
        let mut bodies: Vec<(String, ObjectBody)> = Vec::with_capacity(writing.len());
        // Why one particular record failed, by its record id: the data format conversion
        // reads each record against the bound columns, so what failed is a statement about
        // THAT record (its own failure class, and the column and the value it carried there)
        // rather than one wording for every record of the group.
        let mut faults: BTreeMap<String, RecordFault> = BTreeMap::new();
        for (prefix, records) in writing {
            let (processed, unprocessable) =
                processed_records(&records, target, &mut inflation_budget).await;
            if !unprocessable.is_empty() {
                failed
                    .entry(PROCESSING_FAILED)
                    .or_default()
                    .extend(unprocessable);
            }
            let Some(conversion) = target.conversion.as_ref() else {
                let body = concatenated(&processed, target.append_delimiter);
                // A group whose every record the processing dropped or could not read leaves
                // no object behind: an empty object is not something a delivery writes.
                if !body.is_empty() {
                    bodies.push((prefix, ObjectBody::Records(body)));
                }
                continue;
            };
            match format_conversion::convert(engine, conversion, &processed).await {
                format_conversion::Converted::Rows {
                    body,
                    failed: unconvertible,
                } => {
                    if !unconvertible.is_empty() {
                        let records = failed.entry(FORMAT_CONVERSION_FAILED).or_default();
                        for (record, rejection) in unconvertible {
                            faults
                                .insert(record.record_id.clone(), conversion_fault(&rejection));
                            records.push(record);
                        }
                    }
                    if !body.is_empty() {
                        bodies.push((prefix, ObjectBody::Converted(body)));
                    }
                }
                // No schema to convert against: every record of the group is a failed
                // record, so the whole group goes to the error output rather than being
                // delivered unconverted.
                format_conversion::Converted::NoSchema => {
                    failed
                        .entry(FORMAT_CONVERSION_SCHEMA_MISSING)
                        .or_default()
                        .extend(processed.iter().map(|(record, _)| *record));
                }
                // What this emulator cannot serialize is not a record it drops or
                // mislabels: the whole delivery is owed again instead.
                format_conversion::Converted::Unwritable => return None,
            }
        }
        for (failure, records) in failed {
            // An error output prefix this emulator cannot resolve is not a record it
            // drops: nothing of the delivery is written and the whole of it is owed
            // again, so the records keep their documented retention.
            let Ok(prefix) = error_output_prefix(target, failure, now) else {
                ::tracing::warn!(
                    bucket = %target.bucket,
                    error_code = %failure.error_code,
                    "the destination's error output prefix could not be resolved, so this \
                     delivery was left for a later one"
                );
                return None;
            };
            // A record the data format conversion failed on names the table that conversion
            // was made against; one that failed for any other reason was measured against
            // no table.
            let catalog_table = target
                .conversion
                .as_ref()
                .filter(|_| failure.output_type == FORMAT_CONVERSION_FAILED_OUTPUT_TYPE)
                .map(format_conversion::FormatConversion::data_catalog_table);
            bodies.push((
                prefix,
                ObjectBody::Records(error_payload(
                    &records,
                    failure,
                    now,
                    catalog_table.as_ref(),
                    &faults,
                )),
            ));
        }
        for (prefix, body) in bodies {
            let name = object_name(delivery, now, target.time_zone);
            // An encoder that refused the body is not a record this delivery drops: the
            // whole delivery is owed again instead.
            let encoded = match body {
                ObjectBody::Records(body) => compressed(body, target)?,
                // A converted object is not compressed again: the serializer compressed
                // the object's own blocks (`ParquetSerDe$Compression`), which is the
                // compression a destination converting its data format configures. Its
                // name still carries a configured `FileExtension`, which "will override
                // the default file extension" whatever wrote the object, and its bytes
                // carry no content coding of their own, so it reports none.
                ObjectBody::Converted(body) => Encoded {
                    body,
                    extension: target
                        .file_extension
                        .clone()
                        .filter(|extension| !extension.is_empty())
                        .unwrap_or_default(),
                    content_encoding: None,
                },
            };
            objects.push(DeliveredObject {
                bucket: target.bucket.clone(),
                key: format!("{prefix}{name}{}", encoded.extension),
                body: encoded.body,
                content_encoding: encoded.content_encoding,
                kms_key_arn: target.kms_key_arn.clone(),
                destination: target.destination(),
            });
        }
    }
    Some(objects)
}

/// The bytes one object of a delivery carries, and how its name is settled.
enum ObjectBody {
    /// The records' own data, or the error documents of the records that failed: an object
    /// the destination's `CompressionFormat` encodes.
    Records(Vec<u8>),
    /// The columnar file the destination's data format conversion wrote, which carries its
    /// own compression.
    Converted(Vec<u8>),
}

/// The expanded `ErrorOutputPrefix` a record whose delivery failed is written under.
///
/// "A prefix that Firehose evaluates and adds to failed records before writing them to S3"
/// (`ErrorOutputPrefix`) — with none configured, the error output type is itself that
/// prefix, so a failed record is never written where a delivered one goes.
fn error_output_prefix(
    target: &S3Target,
    failure: DeliveryFailure,
    now: SystemTime,
) -> Result<String, DeliveryFailure> {
    let configured = if target.error_output_prefix.is_empty() {
        format!("{}/", failure.output_type)
    } else {
        target.error_output_prefix.clone()
    };
    expand_prefix(
        &configured,
        &BTreeMap::new(),
        Some(failure.output_type),
        now,
        target.time_zone,
    )
}

/// What an object's records have in common: the partition keys they share, or how their
/// delivery failed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKeys {
    /// The partition keys every record of the group derived. EMPTY for a destination
    /// without dynamic partitioning, where the whole buffer is one group.
    Derived(BTreeMap<String, String>),
    /// How the delivery of every record in the group failed.
    Failed(DeliveryFailure),
}

/// One object's worth of records.
struct RecordGroup<'r> {
    keys: GroupKeys,
    records: Vec<&'r StoredRecord>,
}

/// The records of one delivery, grouped into the objects they are written as.
///
/// Without dynamic partitioning there is one group: "Firehose buffers records before
/// delivering them to the destination", and the buffer is one object. With it, the records
/// are grouped by the partition keys the `MetadataExtractionQuery` derives, so records
/// carrying distinct values land under distinct prefixes — and the records it derived none
/// for are their own group, delivered to the error output prefix.
///
/// `None` when the extraction could not be STARTED, which is not a verdict on the records:
/// the delivery is owed again instead of being written as failures.
async fn grouped<'r>(
    target: &S3Target,
    records: &'r [StoredRecord],
) -> Option<Vec<RecordGroup<'r>>> {
    let Some(query) = target.metadata_extraction_query.clone() else {
        return Some(vec![RecordGroup {
            keys: GroupKeys::Derived(BTreeMap::new()),
            records: records.iter().collect(),
        }]);
    };
    let keys = partition_keys(query, records).await?;
    // Grouped in a `BTreeMap` so the objects of one flush are built in a deterministic
    // order whatever order the records arrived in.
    let mut groups: BTreeMap<GroupKeys, Vec<&StoredRecord>> = BTreeMap::new();
    for (record, keys) in records.iter().zip(keys) {
        groups.entry(keys).or_default().push(record);
    }
    Some(
        groups
            .into_iter()
            .map(|(keys, records)| RecordGroup { keys, records })
            .collect(),
    )
}

/// The partition keys `query` derives from each record, in the records' own order.
///
/// A record the program produced no object of scalars for — unreadable JSON, a program
/// this emulator will not run or cannot compile, an output that is not an object, a value
/// that is not a scalar — is one whose metadata extraction failed, and carries that
/// `error-output-type` instead. Every answer here is one of those two: nothing is left
/// pending on a program, so no caller can wedge the delivery of a stream by configuring
/// one.
///
/// HOW THE RUN IS BOUNDED. The program is run by [`jaq_core`], a maintained jq
/// implementation, because `JsonParsingEngine` names jq (`JQ-1.6`) and a jq program means
/// what jq says it means and not what a hand-written subset of it would. It is
/// CLIENT-supplied, so:
///
/// * STEPS and MEMORY — [`extract_partition_keys`] registers NO library function at all,
///   so the program has no generating or recursive function to call, and
///   [`admissible_program`] refuses the constructs that re-apply an expression or grow a
///   value without bound. One run's values are then at most [`MAX_PROGRAM_TOKENS`] times
///   [`MAX_EXTRACTED_RECORD_BYTES`], and its steps a straight-line walk of the same size:
///   both bounded by the program and the data rather than by a clock. The arithmetic is set
///   out on [`admissible_program`].
/// * DEPTH — [`MAX_PROGRAM_DEPTH`], checked on the TEXT before anything parses it, and the
///   thread's stack is sized for that depth ([`METADATA_EXTRACTION_STACK_BYTES`]).
/// * OUTPUT — only the FIRST output of each record is read, and what it may carry is
///   capped ([`scalar_object`]).
/// * HOST — the engine is given no library functions, no inputs, no variables and no
///   module path, so there is no `env`, `input`, `include`, `debug` or any other
///   host-reaching native in it: a program can reach nothing but the record handed to it.
/// * CANCELLATION — the run reads a flag this waiter sets, between records, and stops
///   there; a run bounded as above cannot be inside one record for long, so this is what
///   ends a run the waiter has given up on rather than leaving it to finish the buffer.
/// * BACKSTOP — the run gets only [`METADATA_EXTRACTION_BUDGET`] of wall clock, on a
///   thread of its own that is never joined (so neither a request nor the shutdown waits
///   on it) and whose permit the WAITER releases. A run that had to be cancelled is charged
///   against [`MAX_ABANDONED_EXTRACTIONS`] for the life of the process, and the program
///   that caused it is never run again, so the feature fails closed instead of letting a
///   caller start one such run per put.
async fn partition_keys(query: String, records: &[StoredRecord]) -> Option<Vec<GroupKeys>> {
    let data: Vec<Vec<u8>> = records.iter().map(|record| record.data.clone()).collect();
    let failed = vec![GroupKeys::Failed(METADATA_EXTRACTION_FAILED); data.len()];
    if let Err(refusal) = admissible_program(&query) {
        ::tracing::warn!(%refusal, "the metadata extraction query is not one this emulator runs");
        return Some(failed);
    }
    if has_overrun(&query) {
        ::tracing::warn!(
            "this metadata extraction query has already overrun its budget once, so it was \
             not run again"
        );
        return Some(failed);
    }
    if abandoned_budget_spent() {
        ::tracing::warn!(
            "this process has abandoned as many metadata extraction runs as it ever will, so \
             no further query is run"
        );
        return Some(failed);
    }
    let Some(permit) = ExtractionPermit::take() else {
        ::tracing::warn!(
            "too many metadata extraction queries are already running, so this one was not \
             started"
        );
        return Some(failed);
    };
    // The program runs on a thread of its own, with a stack sized for the depth bound and a
    // name that identifies it: never a runtime worker, and never joined, so a program that
    // did not come back cannot hold a request, the delivery loop or the shutdown. The run
    // reads `cancelled` between records, so this waiter can end it.
    let cancelled = ::std::sync::Arc::new(::std::sync::atomic::AtomicBool::new(false));
    let (sender, receiver) = ::std::sync::mpsc::channel();
    let program = query.clone();
    let run_cancelled = ::std::sync::Arc::clone(&cancelled);
    let spawned = ::std::thread::Builder::new()
        .name(METADATA_EXTRACTION_THREAD.to_string())
        .stack_size(METADATA_EXTRACTION_STACK_BYTES)
        .spawn(move || {
            let _ = sender.send(extract_partition_keys(&program, &data, &run_cancelled));
        });
    // The handle is KEPT for as long as this waiter runs, so the run is not forgotten the
    // instant it is started; a synchronous run cannot be aborted from outside, which is
    // what `cancelled` above is for.
    let Ok(handle) = spawned else {
        ::tracing::warn!("a thread for the metadata extraction query could not be started");
        return Some(failed);
    };
    let deadline = ::std::time::Instant::now() + METADATA_EXTRACTION_BUDGET;
    loop {
        match receiver.try_recv() {
            // The permit is released HERE, by this waiter, whichever way the wait ends.
            Ok(keys) => return Some(keys),
            // The run ended without an answer: it panicked, which is the program's own
            // failure and not a defect of the records.
            Err(::std::sync::mpsc::TryRecvError::Disconnected) => {
                ::tracing::warn!("the metadata extraction query could not be run");
                return Some(failed);
            }
            Err(::std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if ::std::time::Instant::now() >= deadline {
            cancelled.store(true, ::std::sync::atomic::Ordering::Release);
            let charged = charge_abandoned_extraction();
            ::tracing::warn!(
                charged,
                finished = handle.is_finished(),
                "the metadata extraction query did not come back inside its budget, so it \
                 was cancelled"
            );
            remember_overrun(query);
            drop(permit);
            return Some(failed);
        }
        ::tokio::time::sleep(METADATA_EXTRACTION_POLL).await;
    }
}

/// What an open bracket of a metadata-extraction program is, which is what decides whether
/// a `,` inside it separates ELEMENTS or generates VALUES.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bracket {
    /// A container construction — `{…}`, or a `[…]` that does not follow something
    /// indexable. A `,` inside one separates the elements that container collects, so it
    /// grows the value by the factor [`MAX_PROGRAM_CONSTRUCTIONS`] already bounds.
    Construction,
    /// A group (`(…)`) or an index / slice bracket. A `,` inside one is jq's generator: it
    /// yields two values and a pipe re-applies everything after it once per value.
    Generating,
}

/// Why a metadata-extraction program is not one this emulator runs at all.
///
/// Every check is applied to the TEXT, so no descent over the program has begun when one
/// is refused, and together they are what bound the run's steps, memory and depth
/// ([`partition_keys`]). A program is refused when:
///
/// * it nests deeper than [`MAX_PROGRAM_DEPTH`], or carries more than
///   [`MAX_PROGRAM_TOKENS`] tokens;
/// * it DEFINES a function (`def`), which — with no library function registered — is the
///   one construct left that may call itself, or re-applies an expression once per element
///   of a value: `reduce` and `foreach` re-apply their update expression that way, and a
///   BINDING (`f as $x | g`) re-applies `g` once per output of `f`, which over an iterated
///   path is the same unbounded chain under another spelling;
/// * it carries `+`, `*`, `-` or a lone `/`, the operators that GROW a value out of
///   proportion to the one they were given (`+` concatenates and merges, `*` repeats a
///   string and merges deeply, `-` subtracts one array from another by comparing every pair
///   of their elements, and `/` over two strings SPLITS one into an array of its pieces —
///   one piece per occurrence of the separator), so repeated application of any of them
///   costs far more than the record it started from. None of the four is needed to read a
///   partition key out of a JSON document, which is what a `MetadataExtractionQuery` is
///   for; the DOUBLED `/` — the alternative operator `//`, which takes one of two values
///   and grows neither — is admitted;
/// * it ASSIGNS into a value (`=`, `|=`, `//=`, and any other operator ending in a lone
///   `=`), which embeds a value into a field of itself and chains, so a handful of tokens
///   multiplies the record many times over; the COMPARISONS `==`, `!=`, `<=` and `>=` are
///   admitted, since each yields one boolean whatever it was given;
/// * it carries any punctuation outside the set this accounting can cost — the admitted
///   ones are the forms a program needs to read a partition key out of a JSON document
///   (`.`, `?`, a string literal, an identifier, `[`/`]`, `{`/`}`, `(`/`)`, `|`, `,`, `:`,
///   `$`, `//`, a comparison) — so an operator this scanner was never taught, jq's or a
///   future engine's, is refused rather than silently charged one ordinary token;
/// * it interpolates (`\(…)`), whose contents are an expression this accounting does not
///   read, so brackets, tokens and a definition inside one would be charged nothing;
/// * it carries more than [`MAX_PROGRAM_CONSTRUCTIONS`] container constructions, counted
///   with every `{` and with every `[` that does not follow something INDEXABLE — a
///   keyword is not indexable, so the `[` of `if . then [., .] else . end` is counted as
///   the construction the engine reads it as rather than as an index;
/// * it carries more than [`MAX_PROGRAM_ITERATIONS`] iterations — a `[]` where an index
///   would go, which is jq's GENERATOR over a value's elements — or jq's recursive descent
///   `..`, which generates one output per sub-value at every depth, or a `,` anywhere but
///   inside a container construction, which is jq's own generator of two values (`(.a, .b)`)
///   rather than the separator of the elements `{a:.x,b:.y}` collects. A generator is the
///   one form whose outputs are MANY, and a pipe re-applies everything after it once per
///   output, so generators MULTIPLY what the straight-line accounting below charges for;
/// * it carries a string literal this scanner cannot read as the engine would — one that
///   never closes, or that carries a raw line break jq does not admit — because a scanner
///   that read the text differently from the engine would be shown one program while the
///   engine ran another. A `#` comment runs to the end of its line, exactly as jq reads
///   one, so nothing can be hidden behind one either.
///
/// THE MEMORY BOUND those last three give. With no library function registered there is no
/// recursion; with no definition, no loop form and no binding there is nothing that
/// re-applies an expression over a value, and with at most one generator there is at most
/// one expression whose outputs are many — so a program is STRAIGHT-LINE over at most the
/// elements of ONE value: it either takes the input APART (a path, an index, a slice, one
/// iteration, a comparison, `if`, `//`, `|`) or CONSTRUCTS one container from it. A
/// construction holding `n` copies of its input grows it `n`-fold and a chain of `k`
/// constructions grows it `n^k`-fold — so with `k` capped at one, the generators capped at
/// one (whose outputs together are no larger than the input they came out of) and `n`
/// bounded by the token count, the largest value one run can build is at most
/// [`MAX_PROGRAM_TOKENS`] times [`MAX_EXTRACTED_RECORD_BYTES`], and at most
/// [`MAX_RUNNING_METADATA_EXTRACTIONS`] runs hold one at a time. The steps it takes are
/// bounded the same way: a straight-line walk of at most [`MAX_PROGRAM_TOKENS`] tokens over
/// a value of at most that size.
///
/// A program this refuses is a program whose records go to the error output, which is what
/// Firehose does with a query whose metadata extraction does not succeed.
fn admissible_program(query: &str) -> Result<(), &'static str> {
    // The brackets open right now, innermost last — which is what tells a `,` that SEPARATES
    // the elements of one container from one that GENERATES two values. Its length is the
    // nesting depth, and the depth bound below is enforced as it grows, so it holds at most
    // [`MAX_PROGRAM_DEPTH`] entries however long the text is.
    let mut brackets: Vec<Bracket> = Vec::new();
    let mut tokens = 0usize;
    let mut constructions = 0usize;
    // How many GENERATORS the program carries: an iteration (`[]` where an index would go)
    // yields one output per element of its value, and a pipe re-applies what follows it once
    // per output, so these multiply where a construction merely scales.
    let mut iterations = 0usize;
    // Whether the bracket now open was charged as an INDEX and nothing has been read inside
    // it yet — so the `]` that closes it makes the pair jq's iteration rather than an index
    // or a slice. Cleared by every token read inside the brackets, and carried across
    // whitespace and across a `#` comment, because `.a[ ]` and `.a[ # c\n ]` iterate exactly
    // as `.a[]` does.
    let mut empty_index_bracket = false;
    let mut in_string = false;
    let mut escaped = false;
    let mut word = String::new();
    // Whether the previous token can be INDEXED into, which is what tells `.foo[0]` and
    // `.["a"]` (an index or a slice) from `[.foo]` (a construction).
    let mut indexable = false;
    // Whether a `#` comment is running: jq comments run to the end of the line, so what
    // follows one on that line is not program text — a scanner that missed them could be
    // shown one program and the engine another.
    let mut in_comment = false;
    // Whether a `/` is waiting to be read: a LONE one is division, which over two strings
    // splits one into an array, while a doubled one is the cheap alternative operator `//`.
    // The two are told apart by the character after the first slash, so it is held until
    // then rather than refused or admitted on sight.
    let mut pending_slash = false;
    // The same for an `=` that may still turn out to be the `==` of a comparison: held
    // until the next character says whether it assigns.
    let mut pending_equals = false;
    // The last program character read outside a string or a comment, which is what tells
    // the `=` of `!=`, `<=` and `>=` from the one that assigns.
    let mut previous = '\0';
    for character in query.chars() {
        if in_comment {
            in_comment = character != '\n';
            continue;
        }
        // A `def` inside a string literal defines nothing, and neither does one inside a
        // longer identifier, so the text is read as tokens rather than searched.
        if in_string {
            match character {
                '(' if escaped => return Err("it interpolates"),
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => {
                    in_string = false;
                    indexable = true;
                }
                // A jq string literal does not carry a raw line break (`\n` is how one
                // carries that), so this is text this scanner cannot read as the engine
                // would: refuse rather than guess.
                '\n' | '\r' => return Err("it carries a string this emulator cannot read"),
                _ => {}
            }
            continue;
        }
        if pending_slash {
            pending_slash = false;
            if character == '/' {
                // `//`, the alternative operator: it takes one of two values it is given
                // and grows neither, so it is one ordinary token.
                tokens += 1;
                indexable = false;
                continue;
            }
            return Err("it carries an operator that grows a value");
        }
        if pending_equals {
            pending_equals = false;
            if character == '=' {
                // `==`, a comparison: it yields one boolean whatever it was given.
                tokens += 1;
                indexable = false;
                continue;
            }
            return Err("it assigns into a value");
        }
        let preceding = previous;
        previous = character;
        // Whether the bracket now open was still empty when this character was read: the
        // flag is cleared here and re-set only by the `[` that opens an index bracket and by
        // whitespace inside one, so every other token read inside the brackets ends the
        // empty-bracket form.
        let inside_empty_bracket = empty_index_bracket;
        empty_index_bracket = false;
        if character.is_alphanumeric() || character == '_' {
            word.push(character);
            continue;
        }
        // Every word ENDS here, whatever ended it — a `#` comment and a `"` string
        // literal end one as surely as an operator does, so the word is flushed before
        // either is entered. A word carried across one would be concatenated with the
        // identifier characters that follow it and charged as a single unknown identifier:
        // `inadmissible_word` would no longer recognise the keyword it hides, and because
        // an identifier is indexable where a keyword is not, the `[` after it would be
        // charged as an index rather than as the construction it is — which is the
        // accounting `MAX_PROGRAM_CONSTRUCTIONS` bounds the value of one run with.
        if !word.is_empty() {
            if let Some(refusal) = inadmissible_word(&word) {
                return Err(refusal);
            }
            // A jq KEYWORD is not a value, so nothing can be indexed out of it: the `[`
            // after one opens a construction, which is why a keyword leaves `indexable`
            // off where an identifier or a literal sets it.
            indexable = !is_keyword(&word);
            word.clear();
            tokens += 1;
        }
        if character == '#' {
            in_comment = true;
            // A comment READS nothing, and jq runs one to the end of its line, so it is
            // whitespace to the engine: an index bracket holding only a comment is still
            // the empty-bracket form the engine reads as an iteration. Carried here, and
            // left untouched by the comment's own characters and by the line break that
            // ends it, so `.a[ # c\n ]` is charged as the iteration it is.
            empty_index_bracket = inside_empty_bracket;
            continue;
        }
        if character == '"' {
            in_string = true;
            tokens += 1;
            continue;
        }
        match character {
            '(' | '[' | '{' => {
                tokens += 1;
                // `(` groups rather than constructs, and a `[` right after something
                // indexable is an index, a slice or an ITERATION.
                let bracket = if character == '{' || (character == '[' && !indexable) {
                    constructions += 1;
                    // A `,` inside a construction separates the elements it collects.
                    Bracket::Construction
                } else {
                    // A `,` inside a group or an index bracket yields TWO values, and a
                    // pipe re-applies what follows once per value.
                    Bracket::Generating
                };
                brackets.push(bracket);
                if brackets.len() > MAX_PROGRAM_DEPTH {
                    // The ONE place the depth bound lives, applied as the nesting grows:
                    // nothing is read past it, and the stack above cannot hold more than it
                    // admits.
                    return Err("it nests deeper than this emulator reads");
                }
                // Which of the three that `[` is decided by what closes it: nothing inside
                // it makes the pair jq's iteration.
                empty_index_bracket = character == '[' && indexable;
                indexable = false;
            }
            ')' | ']' | '}' => {
                brackets.pop();
                tokens += 1;
                if character == ']' && inside_empty_bracket {
                    iterations += 1;
                }
                indexable = true;
            }
            // `-` is here for the same reason as `+` and `*`, and a lone `/` for the same
            // reason again: in jq neither is numeric-only — `Str / Str` splits a string
            // into an array and `Array - Array` subtracts one from the other elementwise —
            // so either can hand the engine a value far larger, and a walk far longer, than
            // the record the accounting above charges for. A `/` is held by `pending_slash`
            // above instead, because `//` is not division.
            '+' | '*' | '-' => return Err("it carries an operator that grows a value"),
            '/' => {
                pending_slash = true;
            }
            character if character.is_whitespace() => {
                // Whitespace reads nothing, so an index bracket holding only whitespace is
                // still the empty-bracket form the engine reads as an iteration.
                empty_index_bracket = inside_empty_bracket;
            }
            // jq's recursive descent `..` generates one output per sub-value of the record,
            // at every depth of it, so what follows it in a pipe is re-applied that many
            // times — a generator whose output count is the record's own nesting rather than
            // anything this accounting charges for. Nothing is needed of it to read a
            // partition key out of a document.
            '.' if preceding == '.' => return Err("it descends recursively through a value"),
            '.' | '?' => {
                tokens += 1;
                indexable = true;
            }
            // An `=` completing a comparison is one token; a lone one ASSIGNS, which
            // embeds a value into a field of itself and chains, so it is held until the
            // next character says which it is.
            '=' if matches!(preceding, '!' | '<' | '>') => {
                tokens += 1;
                indexable = false;
            }
            '=' => {
                pending_equals = true;
            }
            // A `,` is jq's own generator wherever it is not separating the elements of a
            // container: `(.a, .b) | f` applies `f` once per value, and a chain of such
            // groups multiplies that per link — the growth the accounting below is written
            // to exclude. Inside a construction it only separates the elements that
            // construction collects, which is what `{dept:.dept,region:.region}` needs.
            ',' if brackets.last() != Some(&Bracket::Construction) => {
                return Err("it generates more than one value")
            }
            // The punctuation a program needs to take a JSON record apart: the pipe, the
            // comma, an object key separator, a binding's `$`, and the two halves of a
            // comparison. Everything else is refused below rather than charged one
            // ordinary token, so an operator this accounting was never taught cannot slip
            // through it.
            '|' | ',' | ':' | '$' | '<' | '>' | '!' => {
                tokens += 1;
                indexable = false;
            }
            _ => return Err("it carries a construct this emulator does not run"),
        }
    }
    if in_string {
        // Text whose string literal never closes is text this scanner cannot read as the
        // engine would, so it is refused rather than admitted on a partial reading.
        return Err("it carries a string this emulator cannot read");
    }
    if pending_slash {
        // A program ending in a single slash carries division, whatever follows it would
        // have been.
        return Err("it carries an operator that grows a value");
    }
    if pending_equals {
        // And one ending in a single `=` assigns, whatever follows it would have been.
        return Err("it assigns into a value");
    }
    if let Some(refusal) = inadmissible_word(&word) {
        return Err(refusal);
    }
    if !word.is_empty() {
        tokens += 1;
    }
    if tokens > MAX_PROGRAM_TOKENS {
        return Err("it carries more tokens than this emulator reads");
    }
    if constructions > MAX_PROGRAM_CONSTRUCTIONS {
        return Err("it constructs more containers than this emulator runs");
    }
    if iterations > MAX_PROGRAM_ITERATIONS {
        return Err("it iterates over a value more than this emulator runs");
    }
    Ok(())
}

/// Why a jq KEYWORD makes a program inadmissible, or `None` for a word that does not.
///
/// The forms whose evaluation is not bounded by the program's own text: a definition may
/// call itself, `reduce` / `foreach` re-apply their update expression once per element of
/// the value they are given, and a BINDING (`f as $x | g`) re-applies `g` once per output
/// of `f` — so over an iterated path (`.items[] as $i | …`) it re-applies it once per
/// element exactly as a loop does, and nesting bindings multiplies that per level. One
/// construction inside any of them is worth an unbounded chain of them, which is what the
/// memory bound [`admissible_program`] documents rests on not happening.
fn inadmissible_word(word: &str) -> Option<&'static str> {
    match word {
        "def" => Some("it defines a function"),
        "reduce" | "foreach" | "as" => Some("it loops over a value"),
        _ => None,
    }
}

/// Whether `word` is one of jq's own KEYWORDS rather than a name, a literal or a function.
///
/// What it decides is the one thing the construction count turns on: a keyword is not a
/// value, so nothing can be indexed out of one, and a `[` that follows it therefore opens
/// an array CONSTRUCTION ("`[ if . then [., .] else . end, . ]`") where the same bracket
/// after an identifier or a literal is an index or a slice. Reading it the other way would
/// leave a construction uncounted, and with it the memory bound
/// [`admissible_program`] documents.
fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "if" | "then"
            | "elif"
            | "else"
            | "end"
            | "try"
            | "catch"
            | "as"
            | "and"
            | "or"
            | "not"
            | "label"
            | "import"
            | "include"
            | "def"
            | "reduce"
            | "foreach"
    )
}

/// Compile `query` once and run it over every record. See [`partition_keys`].
///
/// The engine is built DEFAULT-DENY: `Loader::new([])` gives it no definition to load and
/// `Compiler::default()` registers no native function, so the only things a program can
/// name are jq's own core constructs over the value handed to it — path access, object and
/// array construction, comparison, `if`, `try`, `|` and `,`; the looping forms core jq also
/// has are refused before this runs ([`admissible_program`]). Nothing that reads the
/// environment, a file, an input stream or another module exists in this engine to be
/// called, and nothing that generates values without bound (`range`, `repeat`, `while`,
/// `until`, `recurse`) does either. A program naming one does not compile, and its records
/// go to the error output.
///
/// `cancelled` is read between records, so a waiter that has given up ends the run rather
/// than leaving it to walk the rest of the buffer.
fn extract_partition_keys(
    query: &str,
    data: &[Vec<u8>],
    cancelled: &::std::sync::atomic::AtomicBool,
) -> Vec<GroupKeys> {
    use ::jaq_core::load::{Arena, File, Loader};
    use ::jaq_core::{Compiler, Ctx, RcIter};
    use ::jaq_json::Val;

    let failed = GroupKeys::Failed(METADATA_EXTRACTION_FAILED);
    let arena = Arena::default();
    let loader = Loader::new([]);
    let Ok(modules) = loader.load(&arena, File { code: query, path: () }) else {
        return vec![failed; data.len()];
    };
    let Ok(filter) = Compiler::<_, ::jaq_core::Native<Val>>::default().compile(modules) else {
        return vec![failed; data.len()];
    };
    data.iter()
        .map(|record| {
            if cancelled.load(::std::sync::atomic::Ordering::Acquire) {
                return failed.clone();
            }
            // The other half of the memory bound: what one run can build is a multiple of
            // what it is given, so what it is given is capped. A record past it has no
            // keys derived, like any other record whose extraction did not succeed.
            if record.len() > MAX_EXTRACTED_RECORD_BYTES {
                return failed.clone();
            }
            let derived = (|| {
                let value: ::serde_json::Value = ::serde_json::from_slice(record).ok()?;
                let inputs = RcIter::new(::std::iter::empty());
                let output = filter
                    .run((Ctx::new([], &inputs), Val::from(value)))
                    .next()?
                    .ok()?;
                scalar_object(&::serde_json::Value::from(output))
            })();
            derived.map_or_else(|| failed.clone(), GroupKeys::Derived)
        })
        .collect()
}

/// `value` as a partition-key map: an object whose every value is a scalar, each rendered
/// as the text a prefix carries it as. Anything else answers `None`.
///
/// The OUTPUT cap on a program a caller supplied: a result past
/// [`MAX_PARTITION_KEYS`] keys, or carrying a value past [`MAX_PARTITION_VALUE_BYTES`], is
/// not a partition this delivery takes — so what a program can make the emulator retain is
/// bounded, like the time it gets.
fn scalar_object(value: &::serde_json::Value) -> Option<BTreeMap<String, String>> {
    let object = value.as_object()?;
    if object.len() > MAX_PARTITION_KEYS {
        return None;
    }
    let mut keys = BTreeMap::new();
    for (name, value) in object {
        let rendered = match value {
            ::serde_json::Value::String(text) => text.clone(),
            ::serde_json::Value::Number(number) => number.to_string(),
            ::serde_json::Value::Bool(flag) => flag.to_string(),
            // A null, an array or an object is not a value a prefix can carry.
            _ => return None,
        };
        if name.len() > MAX_PARTITION_VALUE_BYTES || rendered.len() > MAX_PARTITION_VALUE_BYTES {
            return None;
        }
        keys.insert(name.clone(), rendered);
    }
    Some(keys)
}

/// What the destination's data processing left of each record, in arrival order — and,
/// beside it, the records the processing could not read, which the caller writes to the
/// error output instead.
///
/// A record the processing DROPPED (an envelope carrying no log event of the account) is in
/// neither: it was processed, and what processing it left is nothing.
///
/// `budget` is the inflated bytes the rest of this DELIVERY may still hold, charged down as
/// the records are processed, so the sum of what the records of one flush inflate to is
/// bounded and not only each record on its own
/// ([`crate::state::cloudwatch_logs_pipeline::MAX_INFLATED_BYTES_PER_DELIVERY`]).
///
/// Each record is kept BESIDE what processing left of it, because what an object carries is
/// not always those bytes concatenated: a destination converting its data format writes one
/// row per record ([`format_conversion`]) and names the records whose own data could not be
/// converted, which needs the record itself.
///
/// ASYNC because processing is the one CPU-bound step of a delivery: the loop hands the
/// runtime back between records, so a timeout or a cancellation can land while a long buffer
/// is being processed instead of the whole run holding the worker.
async fn processed_records<'r>(
    records: &[&'r StoredRecord],
    target: &S3Target,
    budget: &mut u64,
) -> (Vec<(&'r StoredRecord, Vec<u8>)>, Vec<&'r StoredRecord>) {
    let mut processed = Vec::with_capacity(records.len());
    let mut unprocessable = Vec::new();
    let processing = target.cloudwatch_logs.runs();
    for record in records {
        if processing {
            ::tokio::task::yield_now().await;
        }
        match crate::state::cloudwatch_logs_pipeline::process(
            &record.data,
            target.cloudwatch_logs,
            budget,
        ) {
            Processed::Record(data) => processed.push((*record, data)),
            Processed::Dropped => {}
            Processed::Failed => unprocessable.push(*record),
        }
    }
    (processed, unprocessable)
}

/// The data one object carries when the destination converts nothing: its records in
/// arrival order, each as the destination's data processing left it, with the delimiter that
/// processing asks for between them.
fn concatenated(processed: &[(&StoredRecord, Vec<u8>)], append_delimiter: bool) -> Vec<u8> {
    let mut body = Vec::new();
    for (_, data) in processed {
        body.extend_from_slice(data);
        if append_delimiter {
            body.extend_from_slice(RECORD_DELIMITER);
        }
    }
    body
}

/// The data an ERROR OUTPUT object carries: the error-output document of each record that
/// could not be placed, one per line.
///
/// A record in the error output is not written raw — then nothing would say what failed:
/// Firehose writes a document naming the failure and carrying the record's own bytes, so a
/// consumer of the error prefix can both diagnose and re-drive it. One document per line,
/// so an object holding several is read a document at a time.
///
/// `catalog_table` is the Data Catalog table the failure was measured against, which the
/// document of a record the destination's data format conversion failed on names beside the
/// failure itself ([`format_conversion::FormatConversion::data_catalog_table`]); a failure
/// of any other kind was not measured against a table and names none. `faults` carries, by
/// record id, the code and the message of ONE record where the failure is a statement about
/// that record, which is what its own document names instead of the failure class's.
fn error_payload(
    records: &[&StoredRecord],
    failure: DeliveryFailure,
    now: SystemTime,
    catalog_table: Option<&::serde_json::Value>,
    faults: &BTreeMap<String, RecordFault>,
) -> Vec<u8> {
    let documents: Vec<String> = records
        .iter()
        .map(|record| {
            error_envelope(
                record,
                failure,
                now,
                catalog_table,
                faults.get(&record.record_id),
            )
            .to_string()
        })
        .collect();
    documents.join("\n").into_bytes()
}

/// The error-output document of one record: how the delivery failed, when it was attempted
/// and the record's own data, under the member names Firehose's error output carries
/// (`attemptsMade`, `arrivalTimestamp`, `attemptEndingTimestamp`, `rawData`, and the code
/// and message pair [`DeliveryFailure::reported_member_names`] names).
///
/// `rawData` is the record's data base64-encoded, exactly as the put carried it on the
/// wire, so nothing of the record is lost by the failure. The two timestamps are epoch
/// MILLISECONDS: `arrivalTimestamp` is when the record was ingested and
/// `attemptEndingTimestamp` when this delivery attempt ended. A record the destination's
/// data format conversion failed on also names the Data Catalog table that conversion reads
/// its columns from (`dataCatalogTable`), because WHICH schema the record did not match is
/// what a consumer of that failure diagnoses it by. The code and the message are `fault`'s
/// where the failure is a statement about THIS record's own data, and the failure class's
/// where it is about the whole group.
fn error_envelope(
    record: &StoredRecord,
    failure: DeliveryFailure,
    now: SystemTime,
    catalog_table: Option<&::serde_json::Value>,
    fault: Option<&RecordFault>,
) -> ::serde_json::Value {
    let mut envelope = ::serde_json::json!({
        "attemptsMade": 1,
        "arrivalTimestamp": epoch_millis(record.received_at),
        "attemptEndingTimestamp": epoch_millis(now),
        "rawData": ::aws_smithy_types::base64::encode(&record.data),
    });
    if let Some(members) = envelope.as_object_mut() {
        let (code, reported_message) = failure.reported_member_names();
        let error_code = fault.map_or(failure.error_code, |fault| fault.error_code);
        let message = fault.map_or(failure.error_message, |fault| fault.message.as_str());
        members.insert(code.to_string(), error_code.into());
        members.insert(reported_message.to_string(), message.into());
        if let Some(table) = catalog_table {
            members.insert("dataCatalogTable".to_string(), table.clone());
        }
    }
    envelope
}

/// `at` as epoch milliseconds, which is how the error-output document carries an instant.
fn epoch_millis(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_millis())
        .unwrap_or_default()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// What one object's bytes are delivered as: the encoded body, the file extension its name
/// carries, and the `Content-Encoding` a reader of the object is told the body is in.
struct Encoded {
    body: Vec<u8>,
    /// The extension appended to the object's name, empty where the format carries none and
    /// the destination configures none.
    extension: String,
    /// The content coding the body was encoded with, named as the `CompressionFormat` value
    /// it comes from in lower case, and `None` for a body that was not compressed at all.
    content_encoding: Option<String>,
}

/// `body` encoded as the destination's `CompressionFormat` asks, with the file extension
/// the delivered object's name carries and the content coding the stored object reports.
///
/// `None` when the encoder failed, in which case nothing is delivered for this object
/// rather than a body in a format the destination did not ask for.
fn compressed(body: Vec<u8>, target: &S3Target) -> Option<Encoded> {
    const UNCOMPRESSED: &str = "UNCOMPRESSED";
    let format = target
        .compression_format
        .as_deref()
        .unwrap_or(UNCOMPRESSED);
    let (encoded, default_extension) = match format {
        // "Specify a file extension. It will override the default file extension"
        // (`FileExtension`) — the default being the one the compression format carries:
        // Firehose appends `.gz` to a GZIP object and `.zip` to a ZIP one, and leaves an
        // uncompressed or Snappy object without one.
        "GZIP" => (gzip(&body)?, ".gz"),
        "ZIP" => (zip(&body)?, ".zip"),
        "ZSTD" => (zstd(&body)?, ".zst"),
        "Snappy" => (snappy(&body)?, ""),
        "HADOOP_SNAPPY" => (hadoop_snappy(&body)?, ""),
        UNCOMPRESSED => (body, ""),
        other => {
            ::tracing::warn!(%other, "the destination asks for a compression format this emulator cannot write");
            return None;
        }
    };
    let extension = target
        .file_extension
        .clone()
        .filter(|extension| !extension.is_empty())
        .unwrap_or_else(|| default_extension.to_string());
    Some(Encoded {
        body: encoded,
        extension,
        content_encoding: content_encoding(format),
    })
}

/// The `Content-Encoding` a delivered object whose body was encoded under `format` carries:
/// the content coding that format names, which is the `CompressionFormat` value in lower
/// case, and `None` for `UNCOMPRESSED`, whose body carries no coding at all.
///
/// Firehose states the coding on the object it writes, so a reader handed the bytes knows
/// which decoder they are for — the whole point of the header for a transport that
/// decompresses transparently, and what tells a `zip` object from a `gzip` one.
fn content_encoding(format: &str) -> Option<String> {
    (format != "UNCOMPRESSED").then(|| format.to_ascii_lowercase())
}

/// `body` as a gzip stream.
fn gzip(body: &[u8]) -> Option<Vec<u8>> {
    let mut encoder =
        ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    encoder.write_all(body).ok()?;
    encoder.finish().ok()
}

/// `body` as a zip archive holding it as its single entry.
fn zip(body: &[u8]) -> Option<Vec<u8>> {
    /// The name of the archive's single entry. A ZIP object a Firehose stream delivers is
    /// one buffer's data in one archive, so one entry is what it holds.
    const ENTRY_NAME: &str = "data";
    let mut archive = ::zip::ZipWriter::new(::std::io::Cursor::new(Vec::new()));
    archive
        .start_file(ENTRY_NAME, ::zip::write::SimpleFileOptions::default())
        .ok()?;
    archive.write_all(body).ok()?;
    Some(archive.finish().ok()?.into_inner())
}

/// `body` as a Zstandard frame.
fn zstd(body: &[u8]) -> Option<Vec<u8>> {
    Some(::ruzstd::encoding::compress_to_vec(
        body,
        ::ruzstd::encoding::CompressionLevel::Fastest,
    ))
}

/// `body` in the Snappy framing format.
fn snappy(body: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = ::snap::write::FrameEncoder::new(Vec::new());
    encoder.write_all(body).ok()?;
    encoder.into_inner().ok()
}

/// `body` in the Hadoop Snappy container: the uncompressed length of the stream, then one
/// block as its compressed length followed by raw Snappy, each length a 32-bit big-endian
/// count. That framing — and not the Snappy one above — is what distinguishes
/// `HADOOP_SNAPPY` from `Snappy`.
fn hadoop_snappy(body: &[u8]) -> Option<Vec<u8>> {
    let compressed = ::snap::raw::Encoder::new().compress_vec(body).ok()?;
    let mut framed = Vec::with_capacity(compressed.len() + 8);
    framed.extend_from_slice(&u32::try_from(body.len()).ok()?.to_be_bytes());
    framed.extend_from_slice(&u32::try_from(compressed.len()).ok()?.to_be_bytes());
    framed.extend_from_slice(&compressed);
    Some(framed)
}

/// The name a delivered object carries, under whatever prefix it is written:
/// `<stream>-<version>-<yyyy-MM-dd-HH-mm-ss>-<uuid>`, the documented default form Firehose
/// names an object it delivers to Amazon S3 with — the stream's name, the `VersionId` the
/// configuration it was delivered under carries, the delivery's own second and a value
/// unique to the object, and nothing else between them.
///
/// Two objects of one flush are told apart by that last value alone, which is what carries
/// the per-object uniqueness.
///
/// The stamp is the delivery's instant in the destination's own zone, like every other time
/// field a delivery writes ([`time_zone`]).
fn object_name(delivery: &PendingDelivery, now: SystemTime, zone: DeliveryTimeZone) -> String {
    let stamp = zone.fields(now).map_or_else(String::new, |fields| {
        format!(
            "{}-{}-{}-{}-{}-{}",
            fields.year, fields.month, fields.day, fields.hour, fields.minute, fields.second
        )
    });
    let random = ::uuid::Uuid::new_v4();
    format!(
        "{}-{}-{stamp}-{random}",
        delivery.stream, delivery.version_id
    )
}

/// The prefix a delivered object is written under: `prefix` with its `!{...}` namespaces
/// expanded, plus the automatic time prefix when it carries none.
///
/// Every time field comes from `zone`, the zone the destination's `CustomTimeZone` names
/// ([`time_zone`]).
///
/// The namespaces a prefix may carry, with what each expands to here:
///
/// * `partitionKeyFromQuery:<name>` — the value `keys` holds for `<name>`; a name the
///   query derived no value for makes the whole prefix unresolvable, which is a record
///   whose metadata extraction failed. A partition key is not available for an ERROR OUTPUT
///   prefix — a record is written there BECAUSE its keys could not be placed — so there the
///   expression is carried through as written, like any other namespace this expander does
///   not evaluate: the object reporting the failure is always written rather than the
///   failure being reported by a delivery that can never be made.
/// * `timestamp:<pattern>` — the delivery's own instant, in the destination's zone, under
///   `<pattern>` ([`format_timestamp`]).
/// * `firehose:error-output-type` — why the record is in the error output.
/// * `firehose:random-string` — a value unique to this object.
/// * `partitionKeyFromLambda:<name>` — the keys a Lambda data transformation returned.
///   This emulator runs no transformation, so there are none and the record's partition
///   keys could not be determined, rather than it landing under a prefix with a value
///   invented for it — and in an error output prefix, as above, it is carried through as
///   written.
///
/// "The \"YYYY/MM/DD/HH\" time format prefix is automatically used for delivered Amazon S3
/// files" (`Prefix`), so a prefix carrying no expression at all gets it appended; one that
/// carries an expression says where its objects go itself.
///
/// `Err` carries how the delivery of the records this prefix could not be resolved for
/// failed.
fn expand_prefix(
    prefix: &str,
    keys: &BTreeMap<String, String>,
    error_output_type: Option<&str>,
    now: SystemTime,
    zone: DeliveryTimeZone,
) -> Result<String, DeliveryFailure> {
    const OPEN: &str = "!{";
    let mut expanded = String::new();
    let mut rest = prefix;
    let mut expressions = 0usize;
    while let Some(at) = rest.find(OPEN) {
        let (literal, from_open) = rest.split_at(at);
        expanded.push_str(literal);
        // Everything up to the `!{` is now in `expanded`, so what is still to be read is
        // the text FROM it: a break below leaves exactly that to be appended as written.
        rest = from_open;
        let Some(inside) = from_open.strip_prefix(OPEN) else {
            break;
        };
        let Some(end) = inside.find('}') else {
            // An unclosed expression is not an expression: the text stands as written.
            break;
        };
        let (expression, after) = inside.split_at(end);
        let Some(after) = after.strip_prefix('}') else {
            break;
        };
        rest = after;
        expressions += 1;
        let (namespace, argument) = expression
            .split_once(':')
            .unwrap_or((expression.trim(), ""));
        match (namespace.trim(), argument.trim()) {
            // A partition key resolves only for the prefix of a DELIVERED object. The
            // error output prefix is where a record whose keys could not be placed goes, so
            // asking it for one of those keys there could never be answered: such an
            // expression falls through to the arm that carries an unevaluated namespace
            // through as written, which is what keeps the error output writable.
            ("partitionKeyFromQuery", name) if error_output_type.is_none() => {
                expanded.push_str(keys.get(name).ok_or(METADATA_EXTRACTION_FAILED)?);
            }
            ("partitionKeyFromLambda", _) if error_output_type.is_none() => {
                return Err(METADATA_EXTRACTION_FAILED)
            }
            ("timestamp", pattern) => {
                expanded.push_str(
                    &format_timestamp(now, zone, pattern).ok_or(METADATA_EXTRACTION_FAILED)?,
                );
            }
            ("firehose", "error-output-type") if error_output_type.is_some() => {
                expanded.push_str(error_output_type.unwrap_or_default());
            }
            ("firehose", "random-string") => {
                expanded.push_str(&::uuid::Uuid::new_v4().simple().to_string());
            }
            // A namespace this emulator does not know — and the error output type, in a
            // prefix that is not an error output prefix — is left as the configuration
            // wrote it, so the prefix stays the caller's text rather than silently losing
            // part of it.
            _ => {
                expanded.push_str(OPEN);
                expanded.push_str(expression);
                expanded.push('}');
                expressions -= 1;
            }
        }
    }
    expanded.push_str(rest);
    if expressions == 0 {
        let fields = zone.fields(now).ok_or(METADATA_EXTRACTION_FAILED)?;
        expanded.push_str(&format!(
            "{}/{}/{}/{}/",
            fields.year, fields.month, fields.day, fields.hour
        ));
    }
    Ok(expanded)
}

/// `at` under a `!{timestamp:...}` pattern, in the zone `zone` names.
///
/// The pattern is a Java `DateTimeFormatter` one, of which the fields a delivery prefix is
/// written from are read: `y` (year), `M` (month), `d` (day), `H` (hour of day), `m`
/// (minute) and `s` (second), each repeated as many times as the field is wide, with
/// everything else — separators, literal text — carried through as written. A run of any
/// other pattern letter is carried through as written too, so a prefix this emulator
/// cannot read stays the caller's text rather than becoming a value guessed for it.
fn format_timestamp(at: SystemTime, zone: DeliveryTimeZone, pattern: &str) -> Option<String> {
    let fields = zone.fields(at)?;
    let mut formatted = String::new();
    let mut letters = pattern.chars().peekable();
    while let Some(letter) = letters.next() {
        if !letter.is_ascii_alphabetic() {
            formatted.push(letter);
            continue;
        }
        let mut width = 1usize;
        while letters.peek() == Some(&letter) {
            letters.next();
            width += 1;
        }
        let field = match letter {
            'y' | 'u' => Some(&fields.year),
            'M' => Some(&fields.month),
            'd' => Some(&fields.day),
            'H' => Some(&fields.hour),
            'm' => Some(&fields.minute),
            's' => Some(&fields.second),
            _ => None,
        };
        match field {
            // A field is padded to the pattern's width and, where the pattern is narrower
            // than the field, carries its least significant digits — `yy` is a two-digit
            // year.
            Some(value) => formatted.push_str(&fit(value, width)),
            None => formatted.extend(::std::iter::repeat_n(letter, width)),
        }
    }
    Some(formatted)
}

/// `value` as exactly `width` characters: zero-padded on the left when it is shorter, and
/// its last `width` characters when it is longer.
fn fit(value: &str, width: usize) -> String {
    let held = value.chars().count();
    if held >= width {
        return value.chars().skip(held - width).collect();
    }
    let mut fitted = "0".repeat(width - held);
    fitted.push_str(value);
    fitted
}

/// Start the one in-process task that waits for this store's earliest due delivery,
/// delivers what is then due, and stands down once nothing is owed.
///
/// A producer that puts a record makes no further Firehose call, so a delivery performed
/// only by the next call would never be performed at all — this is what makes the data
/// plane observable without one. It does not change WHEN a delivery happens: the task only
/// calls [`deliver_due`], which applies what the stored due instant has already made due,
/// so a caller reading the destination before that instant still sees the data buffered.
/// The task waits for the clock so that no request ever has to.
///
/// At most one pump runs per store (the store carries the claim), it is bounded by
/// [`MAX_DELIVERY_WAKE_UPS`] wake-ups, and with no async runtime to spawn onto there is no
/// pump — the deliveries are then made by the next operation instead.
///
/// `hop` is how the pump reaches the other services a delivery needs, taken from the request
/// that started the pump: the pump outlives that request, so it holds a clone of the hop
/// rather than borrowing it.
pub fn drive_delivery_clock<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, hop: DeliveryHop) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = start_pump(&mut state.lock()) else {
        return;
    };
    let engine = Arc::clone(engine);
    let pumped = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap is what ended the loop; a pump that ran out of
        // work has already released its claim inside the one critical section that
        // observed it.
        let mut capped = true;
        for _ in 0..MAX_DELIVERY_WAKE_UPS {
            let now = SystemTime::now();
            let Some(due) = next_due_or_release_pump(&mut state.lock(), generation, now) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what keeps a
            // due buffer from costing a CPU-bound spin, and it hands the runtime back
            // between wake-ups.
            let wait = due
                .duration_since(now)
                .unwrap_or_default()
                .clamp(MIN_DELIVERY_WAIT, MAX_DELIVERY_WAIT);
            ::tokio::time::sleep(wait).await;
            // A stream whose data comes from a configured source is fed by that source and
            // by no Firehose call at all, so the wake-up reads what the source has taken
            // since before it delivers what that made due.
            crate::state::kinesis_source::ingest_from_sources(&*engine, SystemTime::now());
            deliver_due(&*engine, SystemTime::now(), Extractions::Run, &hop).await;
        }
        if capped {
            // Only THIS pump's claim, so a pump started since is left holding its own.
            let still_owed = {
                let mut store = state.lock();
                forget_pump(&mut store, generation);
                next_due_or_poll(&store, SystemTime::now()).is_some()
            };
            // A source-fed stream is consumed for as long as it exists and no further
            // Firehose call is coming to start a pump for it, so the cap HANDS OVER rather
            // than standing down on work that is still owed: a fresh pump takes the claim
            // this one released and stands down itself as soon as nothing is owed. The cap
            // still bounds what one task does, which is what it is for.
            if still_owed {
                drive_delivery_clock(&engine, hop);
            }
        }
    });
    // The handle is kept so the pump it replaces is aborted rather than merely forgotten.
    set_pump(&mut pumped.lock(), generation, handle);
}

/// Claim the single delivery pump for this store: the claim's GENERATION when the caller
/// may start one, `None` when one already runs.
fn start_pump(store: &mut ComAmazonawsFirehoseFirehose20150804State) -> Option<u64> {
    if store.delivery_pump_running {
        return None;
    }
    store.delivery_pump_running = true;
    store.delivery_pump_generation = store.delivery_pump_generation.wrapping_add(1);
    Some(store.delivery_pump_generation)
}

/// Release the claim `generation` took — and ONLY that one, so a pump that ended on the
/// wake-up cap cannot clear a claim a pump started since is holding.
fn release_pump(store: &mut ComAmazonawsFirehoseFirehose20150804State, generation: u64) {
    if store.delivery_pump_generation == generation {
        store.delivery_pump_running = false;
    }
}

/// Release the claim `generation` took AND drop the handle stored for it.
///
/// What a pump that is handing over to a fresh one calls: the stored handle is its OWN, and
/// the pump it starts would otherwise abort it as the handle it replaces ([`set_pump`]).
/// Dropping a `JoinHandle` detaches the task rather than cancelling it, so the pump that
/// forgets its handle here still runs to the end of its own body.
fn forget_pump(store: &mut ComAmazonawsFirehoseFirehose20150804State, generation: u64) {
    if store.delivery_pump_generation != generation {
        return;
    }
    release_pump(store, generation);
    store.delivery_pump = None;
}

/// Keep the running pump's task handle, aborting the one it replaces.
///
/// Only when `generation` is still the CURRENT claim. A pump can end before its starter
/// gets here (it releases its own claim as soon as the store owes nothing), and a put that
/// claimed the pump since then has stored a handle of its own: replacing that one would
/// abort a live pump and leave its claim held for ever. So a stale starter installs
/// nothing and aborts nothing — its own task has already released its claim, so it stands
/// down on its own.
fn set_pump(
    store: &mut ComAmazonawsFirehoseFirehose20150804State,
    generation: u64,
    handle: ::tokio::task::JoinHandle<()>,
) {
    if store.delivery_pump_generation != generation {
        return;
    }
    if let Some(previous) = store.delivery_pump.replace(handle) {
        previous.abort();
    }
}

/// What the delivery pump waits for next, or `None` — having RELEASED its claim — when the
/// store owes nothing.
///
/// Observing "nothing is owed" and standing down happen in ONE critical section, which is
/// what makes the hand-over race-free: a put holds this same lock to buffer its record
/// BEFORE it tries to claim the pump, so either this call sees that buffer and the running
/// pump keeps it, or the put's claim finds the flag cleared and starts a pump of its own.
fn next_due_or_release_pump(
    store: &mut ComAmazonawsFirehoseFirehose20150804State,
    generation: u64,
    now: SystemTime,
) -> Option<SystemTime> {
    match next_due_or_poll(store, now) {
        Some(due) => Some(due),
        None => {
            release_pump(store, generation);
            None
        }
    }
}

/// The earliest instant this store owes WORK at: a buffer whose delivery is due, or a poll
/// of the source a stream's configuration names.
///
/// A source-fed stream owes a poll for as long as it exists ([`crate::state::kinesis_source`]),
/// because its data arrives at the source and not through a Firehose call — so a pump keeps
/// running for one even while nothing is buffered.
fn next_due_or_poll(
    store: &ComAmazonawsFirehoseFirehose20150804State,
    now: SystemTime,
) -> Option<SystemTime> {
    [
        next_due_at(store),
        crate::state::kinesis_source::next_poll_at(store, now),
    ]
    .into_iter()
    .flatten()
    .min()
}

#[cfg(test)]
mod tests {
    //! The text accounting a client-supplied `MetadataExtractionQuery` has to pass before
    //! any engine sees it: what [`admissible_program`] admits, and what it refuses so that
    //! the memory and step bound that function documents actually holds.

    use super::{admissible_program, MAX_PROGRAM_CONSTRUCTIONS, MAX_PROGRAM_ITERATIONS};

    #[test]
    fn a_program_that_reads_a_partition_key_out_of_a_record_is_admitted() {
        // The forms a metadata-extraction query needs: an object construction over a path,
        // a nested path, the alternative operator, and each comparison — none of
        // which can hand the engine more than the record it was given.
        for program in [
            "{dept:.dept}",
            "{dept:.dept,region:.region}",
            "{customer:.customer.id}",
            "{dept:(.dept // \"unknown\")}",
            "{dept:(.a[0] // .b[\"k\"] // .c[1:2])}",
            "{dept:(if .a == .b then .a else .b end)}",
            "{dept:(if .a != .b then .a else .b end)}",
            "{dept:(if .a <= .b then .a else .b end)}",
            "{dept:(if .a >= .b then .a else .b end)}",
            "{dept:(try .dept catch .other)}",
            "# a comment\n{dept:.dept}",
        ] {
            assert_eq!(
                admissible_program(program),
                Ok(()),
                "case {program:?}: a program that only reads the record must be admitted"
            );
        }
    }

    #[test]
    fn a_program_that_can_grow_a_value_past_the_documented_bound_is_refused() {
        // (the program, the refusal it must earn). Every row is a way of handing the engine
        // a value, or a walk, larger than the accounting charges for: the four growing
        // operators, every assignment form (which embeds a value into a field of itself and
        // chains), a construction hidden behind a keyword where an index-or-construction
        // heuristic would miss it, and punctuation this accounting was never taught.
        for (program, refusal) in [
            ("{dept:(.a + .b)}", "it carries an operator that grows a value"),
            ("{dept:(.a * 2)}", "it carries an operator that grows a value"),
            ("{dept:(.a - .b)}", "it carries an operator that grows a value"),
            (
                "{dept:(.a / \",\")}",
                "it carries an operator that grows a value",
            ),
            ("{dept:.a} | .b /", "it carries an operator that grows a value"),
            ("{dept:.a} | .b = .", "it assigns into a value"),
            ("{dept:.a} | .b |= .", "it assigns into a value"),
            ("{dept:.a} | .b //= .", "it assigns into a value"),
            ("{dept:.a} | .b =", "it assigns into a value"),
            (
                "[if . then [.,.] else . end,.]",
                "it constructs more containers than this emulator runs",
            ),
            (
                "{dept:(.a @base64)}",
                "it carries a construct this emulator does not run",
            ),
            (
                "{dept:(.a; .b)}",
                "it carries a construct this emulator does not run",
            ),
            ("{dept:(.a % 2)}", "it carries a construct this emulator does not run"),
            ("def f: f; f", "it defines a function"),
            (
                "{dept:(reduce .items[] as $i ([]; $i))}",
                "it loops over a value",
            ),
            // A BINDING re-applies what follows it once per output of what precedes it, so
            // over an iterated path it is a loop under another spelling — and nesting two
            // of them multiplies that per level, which is how a handful of tokens builds a
            // value the straight-line accounting never charged for.
            (". as $record | {dept:$record.dept}", "it loops over a value"),
            (".items[] as $i | {dept:$i}", "it loops over a value"),
            (
                "[.items[] as $i | .items[] as $j | [$i,$j]]",
                "it loops over a value",
            ),
        ] {
            assert_eq!(
                admissible_program(program),
                Err(refusal),
                "case {program:?}: expected the program to be refused as {refusal:?}"
            );
        }
    }

    #[test]
    fn one_construction_is_admitted_whatever_precedes_it() {
        // The construction CAP is what bounds the value a run can build, so the count has
        // to be the one the engine reads: a single array construction is admitted whether
        // it opens the program, follows a keyword, or follows a pipe — and a second one is
        // refused in each of those positions.
        assert_eq!(
            MAX_PROGRAM_CONSTRUCTIONS, 1,
            "these rows are written for a cap of one construction"
        );
        for one in ["[.,.]", "if . then [.,.] else . end", ". | [.,.]"] {
            assert_eq!(
                admissible_program(one),
                Ok(()),
                "case {one:?}: one construction is inside the cap"
            );
        }
        for two in [
            "[[.,.],.]",
            "[if . then [.,.] else . end,.]",
            ". | [.,.] | [.,.]",
            "{a:[.,.]}",
            // A keyword a STRING LITERAL ends is still a keyword, so the construction
            // after it is still a construction: the word is flushed before the literal is
            // entered, and `then"x"else` is two keywords rather than one identifier.
            "[if .a then\"x\"else[.,.,.,.] end,.]",
            // The same where a COMMENT ends the word, which runs to the end of its line.
            "[if .a then # chosen\n[.,.,.,.] else . end,.]",
            "[try\"a\"catch[.,.],.]",
        ] {
            assert_eq!(
                admissible_program(two),
                Err("it constructs more containers than this emulator runs"),
                "case {two:?}: a second construction is past the cap"
            );
        }
    }

    /// RULE: a word is read as the word it is whatever ENDS it — an operator, a string
    /// literal or a comment — so neither can hide a refused word by merging it with the
    /// characters that follow.
    #[test]
    fn a_word_a_string_or_a_comment_ends_is_still_that_word() {
        // (the program, the refusal the word in it earns)
        for (program, refusal) in [
            ("def\"x\"f: .; f", "it defines a function"),
            ("def# a comment\nf: .; f", "it defines a function"),
            ("reduce\"x\" .items[] as $i (.; $i)", "it loops over a value"),
            ("foreach# a comment\n.items[] as $i (.; $i)", "it loops over a value"),
        ] {
            assert_eq!(
                admissible_program(program),
                Err(refusal),
                "case {program:?}: expected the program to be refused as {refusal:?}"
            );
        }
    }

    /// RULE: a GENERATOR hands the engine many outputs and a pipe re-applies everything
    /// after it once per output, so ONE is admitted and a second — or jq's recursive
    /// descent, whose outputs are the record's own sub-values at every depth — is refused.
    #[test]
    fn more_than_one_generator_is_refused() {
        assert_eq!(
            MAX_PROGRAM_ITERATIONS, 1,
            "these rows are written for a cap of one iteration"
        );
        // One iteration, in each position a query needs it, with the index and the slice
        // beside it: only the EMPTY bracket pair is a generator.
        for one in [
            "{dept:.items[]}",
            "[.items[]]",
            ".items[] | {dept:.dept}",
            "{dept:(.a[0] // .b[\"k\"] // .c[1:2])}",
            // Whitespace inside the brackets reads nothing, so this is the same iteration
            // and is admitted as the one this program is allowed.
            "{dept:.items[ ]}",
            // And so does a COMMENT, which jq runs to the end of its line: the brackets
            // hold nothing either way.
            "{dept:.items[ # which\n ]}",
            // A `,` SEPARATING the elements of a construction is not a generator: it is
            // what an object of several partition keys, and an array of several elements,
            // are written with.
            "{dept:.dept,region:.region,year:.year}",
            "[.a,.b,.c]",
        ] {
            assert_eq!(
                admissible_program(one),
                Ok(()),
                "case {one:?}: one generator is inside the cap"
            );
        }
        // (the program, the refusal it must earn). Two iterations MULTIPLY: the engine
        // evaluates what follows once per output of what precedes, so the value built is the
        // product of the two lists' lengths whatever the token count says.
        for (program, refusal) in [
            (
                ".a[] | .b[]",
                "it iterates over a value more than this emulator runs",
            ),
            (
                "[.a[] | .b[]]",
                "it iterates over a value more than this emulator runs",
            ),
            (
                "{dept:(.a[].b[])}",
                "it iterates over a value more than this emulator runs",
            ),
            (
                "{dept:(.a[ ] | .b[\t])}",
                "it iterates over a value more than this emulator runs",
            ),
            // A COMMENT inside the brackets is whitespace to the engine too, so a program
            // hiding its second iteration behind one is still two generators.
            (
                ".a[] | .b[ # c\n ]",
                "it iterates over a value more than this emulator runs",
            ),
            (
                "[.a[] | .b[ # c\n ]]",
                "it iterates over a value more than this emulator runs",
            ),
            // jq's own two-value generator: a `,` outside a container construction. A chain
            // of such groups multiplies per link, which is how a handful of tokens makes the
            // engine materialise far more than the record it started from.
            ("(.,.)|(.,.)|(.,.)", "it generates more than one value"),
            ("[(.,.)|(.,.)]", "it generates more than one value"),
            (".a,.b", "it generates more than one value"),
            ("{dept:(.a,.b)}", "it generates more than one value"),
            // Including one hidden in an INDEX bracket, where two keys yield two values.
            ("{dept:.a[0,1]}", "it generates more than one value"),
            ("[..]", "it descends recursively through a value"),
            (
                "{dept:(.. | .id)}",
                "it descends recursively through a value",
            ),
        ] {
            assert_eq!(
                admissible_program(program),
                Err(refusal),
                "case {program:?}: expected the program to be refused as {refusal:?}"
            );
        }
    }
}
