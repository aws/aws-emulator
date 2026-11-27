// SPDX-License-Identifier: CC0-1.0

//! Fan-out of ingested log events to a log group's subscription-filter destinations.
//!
//! `PutSubscriptionFilter` configures "a real-time stream of log events ingested through
//! PutLogEvents" and has them "delivered to a specific destination", and it states what
//! the receiving service is handed: "When log events are sent to the receiving service,
//! they are Base64 encoded and compressed with the GZIP format." So an event a
//! `PutLogEvents` batch STORES is offered to every subscription filter of its log group,
//! the filter's own `filterPattern` decides which of the batch's events it carries, and
//! the ones it carries travel as gzip-framed JSON — the CloudWatch Logs subscription
//! envelope, whose members are the owner account, the log group, the log stream, the
//! filter name that selected the events and the events themselves ([`envelope`]).
//!
//! The envelope carries each event's message as the filter is configured to see it: as it
//! was INGESTED by default, and — for a filter put with `applyOnTransformedLogs` on a log
//! group that has an active transformer — the version that transformer produced
//! ([`super::transformer`]), which is both what the filter's own `filterPattern` is run
//! against and what the envelope carries, because it is the version the filter was applied
//! to. The event the group STORED is untouched either way, so `GetLogEvents` reports the
//! ingested bytes whether the group transforms or not. A log group's
//! `kmsKeyId` names the key the group's own stored log data is encrypted with
//! (`AssociateKmsKey`: "Associates the specified KMS key with either one log group in the
//! account, or with all stored CloudWatch Logs query insights results in the account"); it
//! is not a property of any subscription destination and no operation copies it to one, so
//! a filter on a group that names a key delivers the same envelope as a filter on a group
//! that names none.
//!
//! The four destination kinds are the ones `PutSubscriptionFilter$destinationArn`
//! documents, and each takes the envelope the way its own API accepts a payload:
//!
//! * a Kinesis data stream — one record per envelope, the gzip frame as its data;
//! * a Firehose delivery stream — one record per envelope, likewise;
//! * a Lambda function — invoked with the `awslogs` document, which carries the gzip frame
//!   base64-encoded, this being the one destination whose payload states that encoding
//!   itself rather than leaving it to a blob member's wire form;
//! * a CloudWatch Logs logical destination created with `PutDestination` — "a logical
//!   object that represents the actual delivery destination", so it is resolved to the
//!   `targetArn` it names ("the physical target where the log events are delivered (for
//!   example, a Kinesis stream)") and the envelope is delivered THERE.
//!
//! Each destination service is reached the way any emulated service reaches another: its
//! AWS SDK client driven over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so nothing here opens a socket
//! or leaves the host and this crate names no other service crate.
//!
//! A delivery is also made AS THIS SERVICE'S OWN SERVICE PRINCIPAL, acting for the log group
//! the events were ingested into ([`as_the_service_principal`]), because one destination
//! asks who is calling: a Firehose delivery stream whose destination enables the CloudWatch
//! Logs pipeline takes records from CloudWatch Logs and from no other producer ("Only
//! requests from CloudWatch Logs are supported when CloudWatch Logs decompression is
//! enabled"), and the credential a call presents is what says whose call it is
//! ([`::service_common::service_principal_calls`]).
//!
//! Every write to a destination is made UNDER A ROLE where the filter's configuration names
//! one, and is authorized against that role before it is attempted
//! ([`withheld_by_delivery_role`]). `PutSubscriptionFilter$roleArn` is "the ARN of an IAM role
//! that grants CloudWatch Logs permissions to deliver ingested log events to the destination
//! stream" and a logical destination carries the same thing for the target it names
//! (`Destination$roleArn`, "a role for impersonation, used when delivering log events to the
//! target"), so the envelope reaches the destination only where the permissions in force on
//! that role allow the destination service's own write action ([`DELIVERY_ACTIONS`]) on the
//! destination resource — the question IAM itself answers
//! ([`service_common::iam_role::role_withholds_action`]). A role that allows it nothing
//! delivers nothing, and the withheld envelope is accounted for exactly as one the
//! destination would not take ([`DELIVERY_ERRORS_METRIC`]), since no record reached the
//! destination either way. A configuration that names NO role — which is every Lambda
//! destination, "you don't need to provide the ARN when you are working with a logical
//! destination for cross-account delivery" — is governed by what authorizes that destination
//! instead, so nothing is withheld here for it.
//!
//! A filter's destination is also written to when the filter is CONFIGURED rather than when
//! anything is ingested: `PutSubscriptionFilter` validates a STREAM destination it was
//! handed by putting one health-check record on it ([`health_check`]), which is the one
//! signal a client has at wiring time that the destination it named is reachable. A Lambda
//! destination is validated by its own resource policy instead and is never invoked by the
//! configuration call ([`CONTROL_MESSAGE_DESTINATIONS`]).
//!
//! A log group's filters are its own AND the one an account-level subscription filter policy
//! configures for it ([`account_level_filter`]): "account-level and log-group-level
//! subscription filters are additive" (`PutAccountPolicy`), and which log groups such a policy
//! configures is its `selectionCriteria`, so a batch ingested into a group the policy selects
//! is fanned out through the policy's destination as well as through the group's own filters.
//!
//! Delivery is best effort, as it is for the real service: `PutLogEvents` "always
//! accepted" the batch, and whether a destination took the envelope is not part of its
//! answer, so a destination that refuses or does not exist leaves the ingestion standing.
//! What it does answer with is TELEMETRY, the only place a client can observe the outcome of
//! a delivery it did not make itself: the events a filter forwarded are counted as the
//! service's own [`FORWARDED_LOG_EVENTS_METRIC`] and a destination that would not take an
//! envelope as [`DELIVERY_ERRORS_METRIC`], both in [`super::emf::LOGS_NAMESPACE`] under the
//! dimensions of the filter that owes the delivery ([`DELIVERY_DIMENSIONS`]), published into
//! the same metric store `ListMetrics` and `GetMetricStatistics` read. A destination that
//! refused is also marked on the filter, which stops attempting it until the next
//! `PutSubscriptionFilter` re-validates it — so the events written during an outage stay
//! readable in the log group and are never re-delivered once the destination returns.
//!
//! What one ingestion can cost is bounded by what the batch and the group already bound: a
//! log group holds at most [`super::MAX_SUBSCRIPTION_FILTERS_PER_GROUP`] filters, so a
//! batch owes at most that many envelopes per record-sized run of its events, and each run
//! is capped at [`MAX_RECORD_BYTES`] of the batch's own bytes
//! ([`record_sized_chunks`]) — one request therefore makes a bounded number of bounded
//! calls.

use std::collections::BTreeMap;
use std::io::Write;

use super::callback_client::callback_client;
use super::emf::{ExtractedDatapoint, DEFAULT_STORAGE_RESOLUTION, LOGS_NAMESPACE};
use super::filter_pattern::FilterPattern;
use super::{ComAmazonawsCloudwatchlogsLogs20140328State, StoredEvent};

/// `messageType` of an envelope carrying ingested log events, as the subscription payload
/// spells it.
pub const MESSAGE_TYPE_DATA: &str = "DATA_MESSAGE";

/// `messageType` of the HEALTH CHECK envelope CloudWatch Logs itself writes to a
/// destination when a subscription filter is configured against it ([`health_check`]): it
/// carries no ingested log event, so it is marked as a message of the control plane rather
/// than one of data.
pub const MESSAGE_TYPE_CONTROL: &str = "CONTROL_MESSAGE";

/// The `owner` a health-check envelope carries. The envelope was written by the SERVICE
/// validating the destination rather than by the ingestion of any account's log events, so
/// it names CloudWatch Logs itself instead of the account that holds the filter — and, for
/// the same reason, the envelope names no log group, no log stream and no subscription
/// filter ([`control_envelope`]).
pub const CONTROL_MESSAGE_OWNER: &str = "CloudwatchLogs";

/// The fixed text a health check's one log event opens with, which is what tells a consumer
/// the record it just read is the service checking the destination rather than log data.
/// The sentence then names the destination being checked
/// ([`CONTROL_MESSAGE_DESTINATIONS`]).
pub const CONTROL_MESSAGE_PREFIX: &str = "CWL CONTROL MESSAGE: Checking health of destination";

/// How the health-check sentence names each kind of destination it is written to, keyed by
/// the kind as [`DESTINATION_TYPE_DIMENSION`] names it: the resource CloudWatch Logs is
/// checking.
///
/// The two STREAM-like kinds only. A Lambda destination is not written to by the put: a
/// function is reached by invoking it, and an invocation of caller code is not something a
/// configuration call performs — what authorizes that destination is the function's own
/// resource policy, which `PutSubscriptionFilter` documents by requiring no `roleArn` for
/// it. So a filter on a Lambda function owes no record here and receives only the envelopes
/// its log group's ingestion delivers.
const CONTROL_MESSAGE_DESTINATIONS: [(&str, &str); 2] = [
    (KINESIS_DESTINATION_TYPE, "Kinesis stream"),
    (FIREHOSE_DESTINATION_TYPE, "Firehose"),
];

/// The digits a delivered event's `id` carries. CloudWatch Logs identifies a log event by
/// an opaque 56-character decimal string; it is minted from the store's own ingestion
/// ordering ([`event_id`]) rather than from anything in the request, so it is unique,
/// stable for the life of the event and monotone in ingestion order.
const EVENT_ID_DIGITS: usize = 56;

/// The widest ingestion sequence an event id reserves room for, as a power of ten: the
/// sequence is the low-order block of [`event_id`] and the ingestion instant the high-order
/// one, so the two never collide.
const EVENT_ID_SEQ_SCALE: u128 = 10_u128.pow(20);

/// The largest payload one delivered record may carry, from the Kinesis
/// `PutRecordsRequestEntry$Data` quota: "The maximum size of the data blob (the payload
/// before base64-encoding) is up to 1 MiB." A batch whose events would not fit is
/// delivered as several envelopes ([`deliveries`]) rather than as one oversized record.
const MAX_RECORD_BYTES: usize = 1_048_576;

/// What one event costs an envelope beyond its message, in bytes: the `id`, the timestamp,
/// the three member names, the quoting and the separators. An over-estimate, so the
/// greedy split in [`deliveries`] never produces a frame past [`MAX_RECORD_BYTES`].
const EVENT_ENVELOPE_OVERHEAD_BYTES: usize = 128;

/// The ARN vendor of each destination kind, as `PutSubscriptionFilter$destinationArn`
/// names them, paired with the ARN resource TYPE that kind's resource carries.
const KINESIS_STREAM: (&str, &str) = ("kinesis", "stream");
const FIREHOSE_DELIVERY_STREAM: (&str, &str) = ("firehose", "deliverystream");
const LAMBDA_FUNCTION: (&str, &str) = ("lambda", "function");
const LOGS_DESTINATION: (&str, &str) = ("logs", "destination");

/// The metric CloudWatch Logs publishes, in [`LOGS_NAMESPACE`], for the log events a
/// subscription filter FORWARDED to its destination: the count of the events the filter's
/// `filterPattern` selected out of the ingested batch, which is what tells a client its
/// pattern is selecting what it intended without reading the destination.
pub const FORWARDED_LOG_EVENTS_METRIC: &str = "ForwardedLogEvents";

/// The metric it publishes, in [`LOGS_NAMESPACE`], for an envelope the filter's destination
/// would not take: the one signal separating a destination that refused a delivery from a
/// delivery that was never attempted.
pub const DELIVERY_ERRORS_METRIC: &str = "DeliveryErrors";

/// The dimension naming the KIND of destination an envelope was delivered to, which both
/// metrics carry beside the log group ([`super::emf::LOG_GROUP_NAME_DIMENSION`]) and the
/// filter ([`FILTER_NAME_DIMENSION`]).
pub const DESTINATION_TYPE_DIMENSION: &str = "DestinationType";

/// The dimension naming the subscription filter that owes the delivery, whose value is that
/// filter's own `filterName`.
pub const FILTER_NAME_DIMENSION: &str = "FilterName";

/// The [`DESTINATION_TYPE_DIMENSION`] value of each destination kind, named by the service
/// that serves the resource the envelope reaches — the three kinds a filter delivers to
/// directly, a logical destination being resolved to one of them
/// ([`physical_destination`]).
const KINESIS_DESTINATION_TYPE: &str = "Kinesis";
const FIREHOSE_DESTINATION_TYPE: &str = "Firehose";
const LAMBDA_DESTINATION_TYPE: &str = "Lambda";

/// The action the destination's own service authorizes the write of one envelope under,
/// per ARN vendor: putting one record on a data stream or a delivery stream, and invoking a
/// function. These are the actions the delivery role has to carry, as `PutDestination$roleArn`
/// states the rule for a stream ("grants CloudWatch Logs permissions to call the Amazon
/// Kinesis PutRecord operation on the destination stream") and as the calls below make them
/// ([`deliver_to_kinesis`], [`deliver_to_firehose`], [`deliver_to_lambda`]).
const DELIVERY_ACTIONS: [(&str, &str); 3] = [
    (KINESIS_STREAM.0, "kinesis:PutRecord"),
    (FIREHOSE_DELIVERY_STREAM.0, "firehose:PutRecord"),
    (LAMBDA_FUNCTION.0, "lambda:InvokeFunction"),
];

/// The unit both delivery metrics' datapoints carry, from the `AWS/Logs` metric table: these
/// are counts of events and of failures rather than measurements in a unit, so the unit is
/// the `StandardUnit` value for no unit.
const DELIVERY_METRIC_UNIT: &str = ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::UNIT_NONE;

/// One envelope a batch of ingested events owes one subscription filter, resolved to the
/// PHYSICAL destination it is delivered to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    /// The log group the events were ingested into, which is one of the dimensions the
    /// delivery's telemetry is published under.
    pub log_group_name: String,
    /// The filter that selected the events: the dimension naming it, and the warning a
    /// failed delivery logs.
    pub filter_name: String,
    /// The ARN of the resource the envelope is delivered to: the filter's own
    /// `destinationArn`, or — for a logical destination — the `targetArn` that destination
    /// names.
    pub destination_arn: String,
    /// The kind of destination that ARN names ([`DESTINATION_TYPE_DIMENSION`]).
    pub destination_type: String,
    /// The IAM role the write to that destination is made under, where the filter's
    /// configuration names one: the filter's own `roleArn`, or — for a logical destination —
    /// the `roleArn` that destination was created with, which is the role it states for
    /// "delivering log events to the target". `None` where no role governs the write, which
    /// is what leaves it unauthorized by this module ([`withheld_by_delivery_role`]).
    pub role_arn: Option<String>,
    /// How many ingested events this envelope carries, which is what a delivery that
    /// succeeds counts as [`FORWARDED_LOG_EVENTS_METRIC`].
    pub forwarded_log_events: usize,
    /// The key the envelope is placed under where the destination partitions what it is
    /// handed. `PutSubscriptionFilter$distribution`: "By default, log data is grouped by
    /// log stream, but the grouping can be set to random for a more even distribution",
    /// so a `ByLogStream` filter keys every envelope on the log stream it came from and a
    /// `Random` one keys each envelope on a value unique to it.
    pub partition_key: String,
    /// The gzip-framed envelope.
    pub payload: Vec<u8>,
}

impl Delivery {
    /// The dimension set this delivery's telemetry is published under: the log group, the
    /// kind of destination and the filter's name — and nothing else, because `PolicyLevel`
    /// is a dimension of an ACCOUNT-level subscription filter and a filter on one log group
    /// has no policy level.
    fn dimensions(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                super::emf::LOG_GROUP_NAME_DIMENSION.to_string(),
                self.log_group_name.clone(),
            ),
            (
                DESTINATION_TYPE_DIMENSION.to_string(),
                self.destination_type.clone(),
            ),
            (FILTER_NAME_DIMENSION.to_string(), self.filter_name.clone()),
        ])
    }

    /// One datapoint of `metric` for this delivery, at the instant the delivery was
    /// attempted: the delivery metrics measure the forwarding, not the log event, so they
    /// fall in the period the forwarding happened in.
    fn datapoint(&self, metric: &str, value: f64, at_ms: i64) -> ExtractedDatapoint {
        ExtractedDatapoint {
            namespace: LOGS_NAMESPACE.to_string(),
            metric_name: metric.to_string(),
            dimensions: self.dimensions(),
            timestamp_ms: at_ms,
            value,
            unit: Some(DELIVERY_METRIC_UNIT.to_string()),
            storage_resolution: DEFAULT_STORAGE_RESOLUTION,
        }
    }
}

/// A delivered log event's id: 56 decimal digits ([`EVENT_ID_DIGITS`]) over the ingestion
/// instant and the ingestion sequence the store stamped the event with, so two events
/// never share one and the order of the ids is the order of ingestion.
pub fn event_id(event: &StoredEvent) -> String {
    let ingested = u128::try_from(event.ingestion_time).unwrap_or(0);
    let value = ingested
        .saturating_mul(EVENT_ID_SEQ_SCALE)
        .saturating_add(u128::from(event.ingestion_seq));
    format!("{value:0>EVENT_ID_DIGITS$}")
}

/// The subscription envelope for `events`, as the receiving service reads it.
fn envelope(
    owner: &str,
    log_group_name: &str,
    log_stream_name: &str,
    filter_name: &str,
    events: &[&StoredEvent],
) -> String {
    ::serde_json::json!({
        "messageType": MESSAGE_TYPE_DATA,
        "owner": owner,
        "logGroup": log_group_name,
        "logStream": log_stream_name,
        "subscriptionFilters": [filter_name],
        "logEvents": events
            .iter()
            .map(|event| ::serde_json::json!({
                "id": event_id(event),
                "timestamp": event.timestamp,
                "message": event.message,
            }))
            .collect::<Vec<::serde_json::Value>>(),
    })
    .to_string()
}

/// `body` in a gzip frame, which is the framing the subscription payload is compressed
/// with.
fn gzip(body: &str) -> Vec<u8> {
    let mut encoder = ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    // Writing to a `Vec` cannot fail, and neither can finishing the frame over one.
    let _ = encoder.write_all(body.as_bytes());
    encoder.finish().unwrap_or_default()
}

/// The HEALTH-CHECK envelope, as the receiving service reads it: the same envelope shape a
/// delivery carries, marked [`MESSAGE_TYPE_CONTROL`] and owned by the service
/// ([`CONTROL_MESSAGE_OWNER`]). It reports no log group, no log stream and no subscription
/// filter, because it is not a reading of any group's log events — it is the service
/// checking that the destination takes a record at all — and its one log event carries the
/// check's own sentence, stamped with the instant the check was made and identifying no
/// stored event.
fn control_envelope(message: &str, at_ms: i64) -> String {
    ::serde_json::json!({
        "messageType": MESSAGE_TYPE_CONTROL,
        "owner": CONTROL_MESSAGE_OWNER,
        "logGroup": "",
        "logStream": "",
        "subscriptionFilters": ::serde_json::Value::Array(Vec::new()),
        "logEvents": [{
            "id": "",
            "timestamp": at_ms,
            "message": message,
        }],
    })
    .to_string()
}

/// The health check a `PutSubscriptionFilter` owes the destination it configures the filter
/// against: ONE record on that destination, written when the filter is configured and
/// before any log event exists, which is how a client confirms at wiring time that the
/// destination it named is reachable.
///
/// Resolved exactly like an ingested batch's deliveries ([`deliveries`]): a logical
/// destination is followed to the physical resource it names, and a destination of no kind
/// a put writes a check to — one that is not a stream, or not a resource a subscription
/// filter delivers to at all — owes no check ([`CONTROL_MESSAGE_DESTINATIONS`]).
pub fn health_check(
    store: &ComAmazonawsCloudwatchlogsLogs20140328State,
    log_group_name: &str,
    filter_name: &str,
    configured_destination_arn: &str,
    role_arn: Option<&str>,
) -> Option<Delivery> {
    let physical_destination_arn = physical_destination(store, configured_destination_arn)?;
    let destination_type = destination_type(&physical_destination_arn)?;
    let payload = control_message(destination_type, super::now_ms())?;
    Some(Delivery {
        log_group_name: log_group_name.to_string(),
        filter_name: filter_name.to_string(),
        destination_arn: physical_destination_arn,
        destination_type: destination_type.to_string(),
        // The check is written to the destination the same way an ingested batch's envelope
        // is, so it is made under the same role and authorized the same way.
        role_arn: delivery_role(store, configured_destination_arn, role_arn),
        // The check carries the service's own sentence, not a log event the filter
        // forwarded, so it forwards nothing.
        forwarded_log_events: 0,
        // The check belongs to no log stream, so the group whose filter is being validated
        // is what it is keyed on.
        partition_key: log_group_name.to_string(),
        payload,
    })
}

/// The TEST MESSAGE CloudWatch Logs writes while it VALIDATES a destination: the health-check
/// envelope above, gzipped the way the receiving service reads a delivered record, naming the
/// kind of destination being checked ([`CONTROL_MESSAGE_DESTINATIONS`]).
///
/// `None` for a kind no record is written to, which is what makes a destination that is not
/// stream-like owe no check and no test message.
fn control_message(destination_type: &str, at_ms: i64) -> Option<Vec<u8>> {
    let named = CONTROL_MESSAGE_DESTINATIONS
        .iter()
        .find(|(kind, _)| *kind == destination_type)
        .map(|(_, named)| *named)?;
    Some(gzip(&control_envelope(
        &format!("{CONTROL_MESSAGE_PREFIX} {named}."),
        at_ms,
    )))
}

/// AWS's refusal for a destination whose TARGET would not take the test message CloudWatch
/// Logs delivers to it while `PutDestination` configures the destination. Fixed text for the
/// class: it names neither the target, nor its vendor, nor anything else from the request.
pub const TEST_MESSAGE_UNDELIVERABLE: &str =
    "Could not deliver test message to specified destination. Check if the destination is \
     valid.";

/// That refusal for the target `target_arn`, or `None` when nothing grounds one.
///
/// `PutDestination` validates the wiring it configures by DELIVERING one test message to the
/// target — the same record a `PutSubscriptionFilter` writes to the destination it configures
/// ([`health_check`]) — so a target its own service reports it holds nothing under is a
/// destination that could never deliver anything, and the put is refused rather than storing
/// it. The write is made to the service that owns the target, over the in-process callback,
/// exactly as a forwarded envelope is.
///
/// The refusal is grounded in the OWNING SERVICE'S OWN ANSWER, and in one answer only: that
/// service reporting the target MISSING. Every other outcome establishes nothing about the
/// target — an emulator assembled with that service absent, a call that could not be made,
/// any other fault of the write — so it refuses nothing, and a target of no stream-like kind
/// owes no test message in the first place.
///
/// `region` is the Region the request was signed for, used only for a `target_arn` that names
/// no Region of its own; the Region that reaches the endpoint is held to the grammar a Region
/// is written in, so no value a request carried can name a host.
pub async fn undeliverable_target_message(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    target_arn: &str,
    partition_key: &str,
) -> Option<&'static str> {
    let destination_type = destination_type(target_arn)?;
    let payload = control_message(destination_type, super::now_ms())?;
    let fallback_region =
        ::service_common::arn::region_label(region).unwrap_or(super::DEFAULT_REGION);
    let target_region = super::arn_region(target_arn)
        .and_then(::service_common::arn::region_label)
        .unwrap_or(fallback_region)
        .to_string();
    if destination_type == KINESIS_DESTINATION_TYPE {
        let stream_name = ::service_common::arn::resource_id_of_type(target_arn, KINESIS_STREAM.1)?;
        let client = callback_client!(aws_sdk_kinesis, ctx.callback(), "kinesis", target_region);
        let answer = client
            .put_record()
            .stream_name(stream_name)
            .partition_key(partition_key)
            .data(::aws_sdk_kinesis::primitives::Blob::new(payload))
            .send()
            .await;
        answer.err().and_then(|error| {
            let reported_absent = error.as_service_error().is_some_and(
                ::aws_sdk_kinesis::operation::put_record::PutRecordError::is_resource_not_found_exception,
            );
            refused_test_message(reported_absent, &error, target_arn)
        })
    } else {
        let stream_name =
            ::service_common::arn::resource_id_of_type(target_arn, FIREHOSE_DELIVERY_STREAM.1)?;
        let record = ::aws_sdk_firehose::types::Record::builder()
            .data(::aws_sdk_firehose::primitives::Blob::new(payload))
            .build()
            .ok()?;
        let client = callback_client!(aws_sdk_firehose, ctx.callback(), "firehose", target_region);
        let answer = client
            .put_record()
            .delivery_stream_name(stream_name)
            .record(record)
            .send()
            .await;
        answer.err().and_then(|error| {
            let reported_absent = error.as_service_error().is_some_and(
                ::aws_sdk_firehose::operation::put_record::PutRecordError::is_resource_not_found_exception,
            );
            refused_test_message(reported_absent, &error, target_arn)
        })
    }
}

/// [`TEST_MESSAGE_UNDELIVERABLE`] when the target's own service reported the target MISSING,
/// and `None` for every other failed write — this emulator failing to make the call, or a
/// fault that says nothing about whether the target is there — which is worth a warning
/// instead. The caller's own value is rendered in its escaped `Debug` form, so an ARN
/// carrying a newline cannot forge a log line.
fn refused_test_message<E, R>(
    reported_absent: bool,
    error: &::aws_sdk_kinesis::error::SdkError<E, R>,
    target_arn: &str,
) -> Option<&'static str>
where
    E: ::std::error::Error + 'static,
    R: ::std::fmt::Debug,
{
    if reported_absent {
        return Some(TEST_MESSAGE_UNDELIVERABLE);
    }
    ::tracing::warn!(
        target_arn = ?target_arn,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "a destination's target did not take the test message, and reported no missing resource"
    );
    None
}

/// The ARN of the resource a `destinationArn` actually delivers to, for a caller OUTSIDE this
/// module: one reading of that question, so the destination a put validates is the one a
/// delivery through the stored filter would reach ([`physical_destination`]).
pub fn physical_destination_arn(
    store: &ComAmazonawsCloudwatchlogsLogs20140328State,
    destination_arn: &str,
) -> Option<String> {
    physical_destination(store, destination_arn)
}

/// AWS's refusal for a subscription destination whose RESOURCE the service that owns it reports
/// it does not hold: the sentence this service states for a destination its test message could
/// not be delivered to at all ([`TEST_MESSAGE_UNDELIVERABLE`]) — the one it answers for the
/// identical condition on the other put that configures a delivery to a stream
/// (`PutDestination`, whose target that service reports missing earns exactly this sentence),
/// naming nothing of the request.
///
/// `PutSubscriptionFilter` validates the destination while it configures the filter — it writes
/// one record to it ([`health_check`]) — so a destination resolving to a stream that does not
/// exist is a `destinationArn` that could never deliver anything, and the put is refused rather
/// than storing a filter whose every delivery would be dropped.
///
/// Only the OWNING SERVICE'S OWN ANSWER grounds the refusal, and only one answer does: that
/// service reporting the resource MISSING. Every other outcome establishes nothing — an
/// emulator assembled with that service absent, a call that could not be made, any other fault
/// — so it refuses nothing, and a destination of no stream-like kind is not asked about at all
/// (a Lambda function is resolved through its own resource policy instead,
/// [`super::lambda_destination`]).
///
/// `region` is the Region the request was signed for, used only for a `destination_arn` that
/// names no Region of its own; the Region that reaches the endpoint is held to the grammar a
/// Region is written in, so no value a request carried can name a host.
pub async fn missing_destination_resource_message(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    destination_arn: &str,
) -> Option<&'static str> {
    let destination_type = destination_type(destination_arn)?;
    let fallback_region =
        ::service_common::arn::region_label(region).unwrap_or(super::DEFAULT_REGION);
    let target_region = super::arn_region(destination_arn)
        .and_then(::service_common::arn::region_label)
        .unwrap_or(fallback_region)
        .to_string();
    if destination_type == KINESIS_DESTINATION_TYPE {
        let stream_name =
            ::service_common::arn::resource_id_of_type(destination_arn, KINESIS_STREAM.1)?;
        let client = callback_client!(aws_sdk_kinesis, ctx.callback(), "kinesis", target_region);
        let answer = client
            .describe_stream_summary()
            .stream_name(stream_name)
            .send()
            .await;
        answer.err().and_then(|error| {
            let reported_absent = error.as_service_error().is_some_and(
                ::aws_sdk_kinesis::operation::describe_stream_summary::DescribeStreamSummaryError::is_resource_not_found_exception,
            );
            refused_destination(reported_absent, &error, destination_arn)
        })
    } else if destination_type == FIREHOSE_DESTINATION_TYPE {
        let stream_name = ::service_common::arn::resource_id_of_type(
            destination_arn,
            FIREHOSE_DELIVERY_STREAM.1,
        )?;
        let client = callback_client!(aws_sdk_firehose, ctx.callback(), "firehose", target_region);
        let answer = client
            .describe_delivery_stream()
            .delivery_stream_name(stream_name)
            .send()
            .await;
        answer.err().and_then(|error| {
            let reported_absent = error.as_service_error().is_some_and(
                ::aws_sdk_firehose::operation::describe_delivery_stream::DescribeDeliveryStreamError::is_resource_not_found_exception,
            );
            refused_destination(reported_absent, &error, destination_arn)
        })
    } else {
        None
    }
}

/// [`TEST_MESSAGE_UNDELIVERABLE`] when the destination's own service reported the resource
/// MISSING, and `None` for every other failed answer — this emulator failing to make the call,
/// or a fault that says nothing about whether the resource is there — which is worth a warning
/// instead. The caller's own value is rendered in its escaped `Debug` form, so an ARN carrying a
/// newline cannot forge a log line.
fn refused_destination<E, R>(
    reported_absent: bool,
    error: &::aws_sdk_kinesis::error::SdkError<E, R>,
    destination_arn: &str,
) -> Option<&'static str>
where
    E: ::std::error::Error + 'static,
    R: ::std::fmt::Debug,
{
    if reported_absent {
        return Some(TEST_MESSAGE_UNDELIVERABLE);
    }
    ::tracing::warn!(
        destination_arn = ?destination_arn,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
        "a subscription destination's own service reported no missing resource for it"
    );
    None
}

/// Every envelope the events `PutLogEvents` just stored on `log_stream_name` owe the
/// subscription filters in force on `log_group_name`, ingested in `region`.
///
/// The filters in force are the group's own and the one an ACCOUNT-LEVEL subscription filter
/// policy configures for it ([`account_level_filter`]): "account-level and log-group-level
/// subscription filters are additive" (`PutAccountPolicy`), so a batch is fanned out through
/// both and neither suppresses the other.
///
/// Read under the store's own guard, so the filters a batch is fanned out through are the
/// ones the group held when the batch arrived. A filter whose pattern matches none of the
/// batch's events owes nothing; neither does a filter whose destination resolves to nothing
/// this emulator can deliver to, nor one whose destination refused its last delivery and has
/// not been re-validated by a `PutSubscriptionFilter` since
/// ([`SubscriptionFilterRecord::destination_unhealthy`](super::SubscriptionFilterRecord)).
pub fn deliveries(
    store: &ComAmazonawsCloudwatchlogsLogs20140328State,
    log_group_name: &str,
    log_stream_name: &str,
    events: &[StoredEvent],
) -> Vec<Delivery> {
    let Some(group) = store.log_groups.get(log_group_name) else {
        return Vec::new();
    };
    let owner = ::engine::emulator_account_id();
    let account_level = account_level_filter(store, group);
    let filters: Vec<&super::SubscriptionFilterRecord> = group
        .subscription_filters
        .values()
        .chain(account_level.iter())
        .collect();
    // The TRANSFORMED version of this batch, materialised once and only where some filter
    // asks for it: a filter put with `applyOnTransformedLogs` "will be applied on the
    // transformed version of the log events instead of the original ingested log events", and
    // an event ingested while no transformer was in force has no transformed version at all
    // ([`StoredEvent::transformed_message`]).
    let transformed: Option<Vec<StoredEvent>> = (events
        .iter()
        .any(|event| event.transformed_message.is_some())
        && filters
            .iter()
            .any(|filter| filter.apply_on_transformed_logs && !filter.destination_unhealthy))
    .then(|| {
        events
            .iter()
            .map(|event| match &event.transformed_message {
                Some(message) => StoredEvent {
                    message: message.clone(),
                    ..event.clone()
                },
                None => event.clone(),
            })
            .collect()
    });
    let mut deliveries = Vec::new();
    for filter in filters {
        if filter.destination_unhealthy {
            continue;
        }
        // A stored filter's pattern was parsed when the put configured it, so one that
        // does not parse here selects nothing rather than refusing the ingestion.
        let Ok(pattern) = FilterPattern::parse(&filter.filter_pattern) else {
            continue;
        };
        // The version of the batch THIS filter sees. Its pattern is run against that version
        // and the envelope carries it, because they are one reading of the event.
        let offered: &[StoredEvent] = match (filter.apply_on_transformed_logs, &transformed) {
            (true, Some(transformed)) => transformed,
            _ => events,
        };
        let matched: Vec<&StoredEvent> = offered
            .iter()
            .filter(|event| pattern.match_message(&event.message).is_some())
            .collect();
        if matched.is_empty() {
            continue;
        }
        let Some(destination_arn) = physical_destination(store, &filter.destination_arn) else {
            continue;
        };
        let Some(destination_type) = destination_type(&destination_arn) else {
            continue;
        };
        for chunk in record_sized_chunks(&matched) {
            let body = envelope(
                owner,
                log_group_name,
                log_stream_name,
                &filter.filter_name,
                chunk,
            );
            // The model's `Distribution` has two values, so a filter that is not grouped
            // by log stream is the one that spreads: each envelope's first event's id is
            // unique to that envelope, so no two of them share a key.
            let partition_key = if filter.distribution == super::DEFAULT_DISTRIBUTION {
                log_stream_name.to_string()
            } else {
                chunk
                    .first()
                    .map_or_else(|| log_stream_name.to_string(), |event| event_id(event))
            };
            deliveries.push(Delivery {
                log_group_name: log_group_name.to_string(),
                filter_name: filter.filter_name.clone(),
                destination_arn: destination_arn.clone(),
                destination_type: destination_type.to_string(),
                role_arn: delivery_role(store, &filter.destination_arn, filter.role_arn.as_deref()),
                forwarded_log_events: chunk.len(),
                partition_key,
                payload: gzip(&body),
            });
        }
    }
    deliveries
}

/// The filter the account's ACCOUNT-LEVEL subscription filter policy configures for one log
/// group, and `None` where no such policy applies to it.
///
/// An account-level policy "applies to all log groups, a subset of log groups, or a data
/// source name and type combination in the account", and which of them it applies to is its
/// `selectionCriteria` ([`super::applicable_account_policy`]) — so a group a
/// `LogGroupName NOT IN [...]` criteria does not exclude is one the policy delivers from. What
/// the policy configures is read from its document ([`super::subscription_filter_policy`]) and
/// delivered through exactly the path a log-group-level filter is: the policy is one filter of
/// the group's, named by the policy's own name, which is the only name a reader of the
/// delivered envelope has for it.
///
/// It carries no delivery health of its own: nothing re-validates an account-level policy's
/// destination the way a `PutSubscriptionFilter` re-validates a filter's, so a destination
/// that refused one batch is attempted again on the next.
fn account_level_filter(
    store: &ComAmazonawsCloudwatchlogsLogs20140328State,
    group: &super::LogGroupRecord,
) -> Option<super::SubscriptionFilterRecord> {
    let policy = super::applicable_account_policy(
        &store.account_policies,
        super::POLICY_TYPE_SUBSCRIPTION_FILTER,
        group,
    )?;
    let configured = super::subscription_filter_policy(&policy.policy_document)?;
    Some(super::SubscriptionFilterRecord {
        filter_name: policy.policy_name.clone(),
        filter_pattern: configured.filter_pattern,
        destination_arn: configured.destination_arn,
        role_arn: configured.role_arn,
        distribution: configured.distribution,
        // The attributes an account-level policy's document names are the four
        // `PutAccountPolicy$policyDocument` lists for one, so nothing in it configures the
        // members a log-group-level put states beyond them.
        apply_on_transformed_logs: false,
        field_selection_criteria: None,
        emit_system_fields: None,
        creation_time_ms: policy.last_updated_time_ms,
        destination_unhealthy: false,
    })
}

/// `matched` split into runs small enough that each one's envelope fits a single delivered
/// record ([`MAX_RECORD_BYTES`]). A single event larger than that is still delivered on its
/// own — the destination refuses it rather than this emulator dropping it silently.
fn record_sized_chunks<'a, 'e>(matched: &'a [&'e StoredEvent]) -> Vec<&'a [&'e StoredEvent]> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut bytes = 0;
    for (index, event) in matched.iter().enumerate() {
        let cost = event.message.len() + EVENT_ENVELOPE_OVERHEAD_BYTES;
        if index > start && bytes + cost > MAX_RECORD_BYTES {
            chunks.push(&matched[start..index]);
            start = index;
            bytes = 0;
        }
        bytes += cost;
    }
    if start < matched.len() {
        chunks.push(&matched[start..]);
    }
    chunks
}

/// The ARN of the resource a `destinationArn` actually delivers to: the value itself for a
/// destination that IS a physical resource, and the `targetArn` of the stored logical
/// destination for one that names a `PutDestination` destination. `None` when the value
/// names a logical destination this account does not hold, or a resource of no kind this
/// operation delivers to.
fn physical_destination(
    store: &ComAmazonawsCloudwatchlogsLogs20140328State,
    destination_arn: &str,
) -> Option<String> {
    let parsed = ::service_common::arn::parse(destination_arn)?;
    if parsed.vendor == LOGS_DESTINATION.0 {
        // Resolved through the store's own lookup, so the destination a delivery follows is
        // the one `PutSubscriptionFilter` authorized the filter against.
        let target = store.destination_by_arn(destination_arn)?.target_arn.clone();
        // A logical destination's target is a physical resource of one of the streaming
        // vendors, so it is not followed further: a destination naming another destination
        // addresses nothing.
        return is_deliverable(&target).then_some(target);
    }
    is_deliverable(destination_arn).then(|| destination_arn.to_string())
}

/// The IAM role the write to `destination_arn` is made under, as the stored configuration
/// names it: the `roleArn` of the LOGICAL destination for a `destinationArn` that names one —
/// that is the role that destination states for "delivering log events to the target" and the
/// filter carries none for it — and otherwise the role the filter itself was configured with.
///
/// `None` where nothing names a role, so nothing here is authorized against one: a filter put
/// without a `roleArn` (a Lambda destination), a logical destination this account does not
/// hold, or one stored with an empty role.
fn delivery_role(
    store: &ComAmazonawsCloudwatchlogsLogs20140328State,
    destination_arn: &str,
    filter_role_arn: Option<&str>,
) -> Option<String> {
    let configured = if ::service_common::arn::parse(destination_arn)
        .is_some_and(|parsed| parsed.vendor == LOGS_DESTINATION.0)
    {
        store
            .destination_by_arn(destination_arn)
            .map(|destination| destination.role_arn.clone())
    } else {
        filter_role_arn.map(::std::string::ToString::to_string)
    };
    configured.filter(|role_arn| !role_arn.trim().is_empty())
}

/// Whether an ARN names a physical resource this module delivers an envelope to.
fn is_deliverable(arn: &str) -> bool {
    [KINESIS_STREAM, FIREHOSE_DELIVERY_STREAM, LAMBDA_FUNCTION]
        .iter()
        .any(|(vendor, resource_type)| {
            ::service_common::arn::parse(arn).is_some_and(|parsed| parsed.vendor == *vendor)
                && ::service_common::arn::resource_id_of_type(arn, resource_type)
                    .is_some_and(|id| !id.is_empty())
        })
}

/// The kind of destination an ARN names, as the [`DESTINATION_TYPE_DIMENSION`] of the
/// delivery's telemetry names it: the service that serves the resource the envelope reaches.
/// `None` for an ARN of no kind a subscription filter delivers to.
///
/// Visible to the rest of this service's state because the refusal a put answers when it cannot
/// assume the configured delivery role names the same kind
/// ([`super::delivery_role`]): one reading of the vendor-to-kind question, so the two cannot
/// disagree about what a destination is called.
pub(super) fn destination_type(arn: &str) -> Option<&'static str> {
    let vendor = ::service_common::arn::parse(arn)?.vendor.to_string();
    [
        (KINESIS_STREAM.0, KINESIS_DESTINATION_TYPE),
        (FIREHOSE_DELIVERY_STREAM.0, FIREHOSE_DESTINATION_TYPE),
        (LAMBDA_FUNCTION.0, LAMBDA_DESTINATION_TYPE),
    ]
    .into_iter()
    .find(|(named, _)| *named == vendor)
    .map(|(_, kind)| kind)
}

/// Hand every envelope of `deliveries` to its destination ([`hand_over`]), and publish what
/// each delivery did.
///
/// `fallback_region` is the Region the ingesting request was signed for, used only for a
/// destination ARN that names no Region of its own.
///
/// The ingestion stands whatever the destinations answer — `PutLogEvents` already answered
/// the batch — so the outcome is reported as the service's own telemetry instead: the events
/// an accepted envelope carried count as [`FORWARDED_LOG_EVENTS_METRIC`], an envelope a
/// destination would not take counts as [`DELIVERY_ERRORS_METRIC`], and the filter whose
/// destination refused is marked so nothing is attempted against it until the next
/// `PutSubscriptionFilter`. Both the metric store and the log store are reached with no
/// guard held across any in-process call.
pub async fn deliver<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    fallback_region: &str,
    deliveries: Vec<Delivery>,
) {
    let mut datapoints: Vec<ExtractedDatapoint> = Vec::with_capacity(deliveries.len());
    let mut refused: Vec<(String, String)> = Vec::new();
    for delivery in deliveries {
        if ::service_common::arn::parse(&delivery.destination_arn).is_none() {
            continue;
        }
        let failure = hand_over(engine, ctx, fallback_region, &delivery).await;
        let at_ms = super::now_ms();
        match failure {
            None => datapoints.push(delivery.datapoint(
                FORWARDED_LOG_EVENTS_METRIC,
                // The datapoint counts the events the filter's pattern selected and this
                // envelope carried, not the events the batch wrote.
                delivery.forwarded_log_events as f64,
                at_ms,
            )),
            Some(failure) => {
                // One envelope the destination would not take is one delivery error. The
                // destination answering at all is its refusal, so the filter stops
                // attempting it until a put re-validates it; the events it carried stay in
                // the log group and are never re-delivered.
                datapoints.push(delivery.datapoint(DELIVERY_ERRORS_METRIC, 1.0, at_ms));
                refused.push((
                    delivery.log_group_name.clone(),
                    delivery.filter_name.clone(),
                ));
                ::tracing::warn!(
                    filter_name = %delivery.filter_name,
                    destination_arn = %delivery.destination_arn,
                    error = %failure,
                    "a subscription filter's destination did not take the delivered log events"
                );
            }
        }
    }
    super::emf::publish(engine, &datapoints);
    mark_destinations_unhealthy(engine, &refused);
}

/// Hand one health check ([`health_check`]) to the destination a `PutSubscriptionFilter`
/// just configured a filter against.
///
/// The check is the service validating the destination, not a forwarding of any log event,
/// so it publishes neither delivery metric and marks nothing: a destination that would not
/// take the check has forwarded no event and lost none, and the next ingested batch is
/// attempted against it exactly as it would have been. What a destination that refuses
/// leaves behind is the warning below, and the absence of the record a client polls for.
pub async fn deliver_health_check<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    fallback_region: &str,
    health_check: Delivery,
) {
    if let Some(failure) = hand_over(engine, ctx, fallback_region, &health_check).await {
        ::tracing::warn!(
            filter_name = %health_check.filter_name,
            destination_arn = %health_check.destination_arn,
            error = %failure,
            "a subscription filter's destination did not take the health check"
        );
    }
}

/// Hand one envelope to the destination its ARN names, answering why the destination would
/// not take it and `None` when it took it.
///
/// Every delivery goes to a service of THIS emulator: the vendor chooses which of the
/// clients below is used, and the Region label is held to the grammar a Region is written
/// in ([`service_common::arn::region_label`]) before it is put in an endpoint, so no value
/// a request carried can name a host. `fallback_region` is the Region the calling request
/// was signed for, used only for a destination ARN that names no Region of its own.
async fn hand_over<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    fallback_region: &str,
    delivery: &Delivery,
) -> Option<String> {
    let fallback_region =
        ::service_common::arn::region_label(fallback_region).unwrap_or(super::DEFAULT_REGION);
    let parsed = ::service_common::arn::parse(&delivery.destination_arn)?;
    let vendor = parsed.vendor.to_string();
    // The Region the destination is addressed in comes from the destination's own ARN,
    // which a request carried, so it is held to the grammar a Region is written in before
    // it reaches an endpoint — the same reading the fallback went through above. A value
    // that is not a Region label addresses nothing, so the Region the calling request was
    // signed for stands instead.
    let region = ::service_common::arn::region_of(&delivery.destination_arn, &vendor)
        .and_then(::service_common::arn::region_label)
        .unwrap_or(fallback_region)
        .to_string();
    // The write is made UNDER the configured delivery role, so what that role is allowed to
    // do on the destination decides whether it is made at all: a role the destination's
    // service would refuse the call to writes nothing, and the envelope is accounted for as
    // one the destination did not take.
    if let Some(withheld) = withheld_by_delivery_role(ctx, &region, delivery, &vendor).await {
        return Some(withheld);
    }
    if vendor == KINESIS_STREAM.0 {
        deliver_to_kinesis(ctx, &region, delivery).await
    } else if vendor == FIREHOSE_DELIVERY_STREAM.0 {
        deliver_to_firehose(engine, ctx, &region, fallback_region, delivery).await
    } else if vendor == LAMBDA_FUNCTION.0 {
        deliver_to_lambda(ctx, &region, delivery).await
    } else {
        Some("the destination names no kind a subscription filter delivers to".to_string())
    }
}

/// Why the DELIVERY ROLE withholds this envelope, and `None` where it does not withhold it —
/// which is also the answer where no role governs the write at all.
///
/// CloudWatch Logs writes to a subscription destination under the role the configuration
/// names, so the write is the role's to make: it happens only where the permissions in force
/// on that role allow the destination service's own write action for the vendor the envelope
/// is going to ([`DELIVERY_ACTIONS`]) on the destination resource. That decision is IAM's,
/// asked of IAM ([`service_common::iam_role::role_withholds_action`]), and a role IAM
/// withholds the action from delivers nothing — never a record the destination's service
/// would have refused.
///
/// `None`, so the delivery proceeds, where nothing says otherwise: a configuration naming no
/// role, a vendor with no write action of this kind, and every case IAM gave no answer about.
async fn withheld_by_delivery_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    delivery: &Delivery,
    vendor: &str,
) -> Option<String> {
    let role_arn = delivery.role_arn.as_deref()?;
    let action = DELIVERY_ACTIONS
        .iter()
        .find(|(named, _)| *named == vendor)
        .map(|(_, action)| *action)?;
    ::service_common::iam_role::role_withholds_action(
        ctx,
        region,
        role_arn,
        action,
        &delivery.destination_arn,
    )
    .await
    .then(|| {
        format!(
            "the delivery role is not allowed to {action} on the destination, so no record \
             was written to it"
        )
    })
}

/// Mark each named filter's destination as one that refused its last delivery, so
/// [`deliveries`] attempts nothing against it until a `PutSubscriptionFilter` re-validates
/// it. A filter deleted or re-put since the attempt is left alone: the mark belongs to the
/// delivery that failed, not to whatever the group holds afterwards.
fn mark_destinations_unhealthy<Eng: Send + Sync + 'static>(
    engine: &Eng,
    refused: &[(String, String)],
) {
    if refused.is_empty() {
        return;
    }
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    for (log_group_name, filter_name) in refused {
        if let Some(filter) = store
            .log_groups
            .get_mut(log_group_name)
            .and_then(|group| group.subscription_filters.get_mut(filter_name))
        {
            filter.destination_unhealthy = true;
        }
    }
}

/// How a failed delivery is worded in the warning: the SDK's own rendering of the answer
/// the destination service gave.
fn why(error: &impl std::error::Error) -> String {
    ::aws_smithy_types::error::display::DisplayErrorContext(error).to_string()
}

/// Deliver one envelope to a Kinesis data stream, as one record keyed on the delivery's
/// partition key.
async fn deliver_to_kinesis(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    delivery: &Delivery,
) -> Option<String> {
    let stream_name =
        ::service_common::arn::resource_id_of_type(&delivery.destination_arn, KINESIS_STREAM.1)?;
    let client = callback_client!(aws_sdk_kinesis, ctx.callback(), "kinesis", region);
    client
        .put_record()
        .stream_name(stream_name)
        .partition_key(&delivery.partition_key)
        .data(::aws_sdk_kinesis::primitives::Blob::new(
            delivery.payload.clone(),
        ))
        .send()
        .await
        .err()
        .map(|error| why(&error))
}

/// Deliver one envelope to a Firehose delivery stream, as one record.
///
/// Made AS THIS SERVICE'S OWN SERVICE PRINCIPAL, acting for the log group the events were
/// ingested into ([`as_the_service_principal`]): a delivery stream whose destination enables
/// the CloudWatch Logs pipeline takes records from CloudWatch Logs and from no other
/// producer ("Only requests from CloudWatch Logs are supported when CloudWatch Logs
/// decompression is enabled"), and the credential the call presents is what says whose call
/// it is. A call that cannot be made as that principal is made the ordinary way — as the
/// in-process caller — which is what every destination that does not ask who is calling
/// takes.
async fn deliver_to_firehose<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    log_group_region: &str,
    delivery: &Delivery,
) -> Option<String> {
    let stream_name = ::service_common::arn::resource_id_of_type(
        &delivery.destination_arn,
        FIREHOSE_DELIVERY_STREAM.1,
    )?;
    let in_flight = as_the_service_principal(engine, ctx, log_group_region, delivery);
    let client = match &in_flight {
        // The credential this ONE call was minted, in place of the callback's own: the
        // macro applies a trailing setting after the credentials provider it configures
        // itself, so this is the provider the client signs with, and the call then carries
        // the access key id the directory of in-flight service-principal calls holds its
        // record under.
        Some(in_flight) => callback_client!(
            aws_sdk_firehose,
            ctx.callback(),
            "firehose",
            region,
            credentials_provider(::aws_sdk_firehose::config::Credentials::new(
                in_flight.access_key_id.clone(),
                in_flight.secret_access_key.clone(),
                None,
                None,
                SERVICE_PRINCIPAL,
            )),
        ),
        None => callback_client!(aws_sdk_firehose, ctx.callback(), "firehose", region),
    };
    let record = ::aws_sdk_firehose::types::Record::builder()
        .data(::aws_sdk_firehose::primitives::Blob::new(
            delivery.payload.clone(),
        ))
        .build();
    match record {
        Ok(record) => client
            .put_record()
            .delivery_stream_name(stream_name)
            .record(record)
            .send()
            .await
            .err()
            .map(|error| why(&error)),
        Err(error) => Some(why(&error)),
    }
}

/// The service principal this service forwards a subscription filter's log events as, which
/// is the principal a filter's delivery role trusts (`PutSubscriptionFilter$roleArn`, "an IAM
/// role that grants CloudWatch Logs permissions to deliver ingested log events to the
/// destination stream").
pub const SERVICE_PRINCIPAL: &str = "logs.amazonaws.com";

/// How long a record of one in-flight delivery stays readable.
///
/// What ENDS a record is the guard being dropped, which happens on every path out of the
/// delivery; the deadline is the floor that bounds a record nothing dropped (see
/// [`::service_common::service_principal_calls`]). It sits well past one in-process call, so
/// a record can never expire while the destination's own handler is still deciding the call.
const IN_FLIGHT_RECORD_SLACK: ::std::time::Duration = ::std::time::Duration::from_secs(60);

/// Mint the credential one delivery is made under and record, under its access key id, that
/// this is a call of [`SERVICE_PRINCIPAL`] acting for the log group the events came from.
///
/// The service SERVING the delivery reads that record for the access key id the call presents
/// and so decides it for this service principal rather than for whoever the in-process
/// credential speaks for. `None` where the call cannot be made as the principal at all — an
/// engine carrying no such directory, a store that minted no secret, a directory already
/// holding as many in-flight calls as it records — in which case the caller makes the
/// ordinary in-process call instead: the narrowest identity available, and the one every
/// destination that does not ask who is calling already took.
fn as_the_service_principal<'e, Eng: Send + Sync + 'static>(
    engine: &'e Eng,
    ctx: &'e smithy_server_runtime::OperationContext<'_>,
    log_group_region: &str,
    delivery: &Delivery,
) -> Option<InFlightDelivery<'e, Eng>> {
    let directory = in_flight_calls(engine)?;
    let minted = ctx
        .credential_store()
        .create(&::emulator_server::aws_credentials::AWS_ACCESS_KEY);
    let secret_access_key = minted
        .secret(::emulator_server::aws_credentials::SECRET_ACCESS_KEY)
        .map(str::to_string);
    let now = ::std::time::SystemTime::now();
    let recorded = secret_access_key.is_some()
        && directory.lock().remember(
            minted.id.clone(),
            ::service_common::service_principal_calls::ServicePrincipalCall {
                principal: SERVICE_PRINCIPAL.to_string(),
                // The resource the forwarding is made ON BEHALF OF: the log group whose
                // ingested events this envelope carries.
                source_arn: super::log_group_arn(log_group_region, &delivery.log_group_name, false),
                source_account: ::engine::emulator_account_id().to_string(),
                expires_at: now + IN_FLIGHT_RECORD_SLACK,
            },
            now,
        );
    match secret_access_key.filter(|_| recorded) {
        Some(secret_access_key) => Some(InFlightDelivery {
            engine,
            credentials: ctx.credential_store(),
            access_key_id: minted.id.clone(),
            secret_access_key,
        }),
        None => {
            let _ = ctx.credential_store().delete(&minted.id);
            None
        }
    }
}

/// One delivery's own credential and in-flight record, held for as long as that delivery is
/// being made.
///
/// Dropping it forgets the record and DELETES the credential, so the identity the call was
/// made under exists only while the destination is answering it: a credential nothing holds
/// can sign nothing, and a record nothing holds decides nothing.
struct InFlightDelivery<'e, Eng: Send + Sync + 'static> {
    engine: &'e Eng,
    credentials: &'e ::emulator_server::CredentialStore,
    access_key_id: String,
    secret_access_key: String,
}

impl<Eng: Send + Sync + 'static> Drop for InFlightDelivery<'_, Eng> {
    fn drop(&mut self) {
        if let Some(directory) = in_flight_calls(self.engine) {
            directory.lock().forget(&self.access_key_id);
        }
        let _ = self.credentials.delete(&self.access_key_id);
    }
}

/// The shared directory of in-flight service-principal calls, in the one shared engine:
/// written here and read by the service serving the delivery.
fn in_flight_calls<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<::service_common::service_principal_calls::ServicePrincipalCalls>,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::service_principal_calls::ServicePrincipalCalls,
            >,
        )
}

/// Deliver one envelope to a Lambda function, as the `awslogs` document a subscription
/// filter's Lambda destination is invoked with.
///
/// The invocation is asynchronous (`InvocationType` `Event`): the function's answer is
/// nothing the ingesting request reports, so nothing waits for it.
async fn deliver_to_lambda(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    delivery: &Delivery,
) -> Option<String> {
    let function =
        ::service_common::arn::resource_id_of_type(&delivery.destination_arn, LAMBDA_FUNCTION.1)?;
    let payload = ::serde_json::json!({
        "awslogs": {"data": ::aws_smithy_types::base64::encode(&delivery.payload)},
    })
    .to_string();
    let client = callback_client!(aws_sdk_lambda, ctx.callback(), "lambda", region);
    client
        .invoke()
        .function_name(function)
        .invocation_type(::aws_sdk_lambda::types::InvocationType::Event)
        .payload(::aws_smithy_types::Blob::new(payload.into_bytes()))
        .send()
        .await
        .err()
        .map(|error| why(&error))
}

#[cfg(test)]
mod tests {
    use super::{event_id, record_sized_chunks, EVENT_ID_DIGITS, MAX_RECORD_BYTES};
    use crate::state::StoredEvent;

    fn stored(message: &str, ingestion_seq: u64) -> StoredEvent {
        StoredEvent {
            timestamp: 1_700_000_000_000,
            message: message.to_string(),
            masked_message: None,
            transformed_message: None,
            ingestion_time: 1_700_000_000_123,
            ingestion_seq,
            ingest_kms_key_id: None,
        }
    }

    /// Every id is 56 decimal digits, and two events ingested one after the other get
    /// different ids in ingestion order.
    #[test]
    fn an_event_id_is_a_56_digit_decimal_unique_to_the_event() {
        let first = event_id(&stored("a", 1));
        let second = event_id(&stored("a", 2));
        for (case, id) in [("first", &first), ("second", &second)] {
            assert_eq!(
                id.len(),
                EVENT_ID_DIGITS,
                "case {case}: an event id carries {EVENT_ID_DIGITS} digits, got {id}"
            );
            assert!(
                id.chars().all(char::is_numeric),
                "case {case}: an event id is decimal, got {id}"
            );
        }
        assert!(
            first < second,
            "case ordering: ids follow ingestion order, got {first} then {second}"
        );
    }

    /// A batch whose events would not fit one record is split, and a batch that fits is
    /// not.
    #[test]
    fn events_are_split_into_record_sized_runs() {
        let small: Vec<StoredEvent> = (0..4).map(|seq| stored("x", seq)).collect();
        let borrowed: Vec<&StoredEvent> = small.iter().collect();
        let chunks = record_sized_chunks(&borrowed);
        assert_eq!(
            chunks.len(),
            1,
            "case small: four tiny events fit one record, got {} runs",
            chunks.len()
        );

        let big_message = "y".repeat(MAX_RECORD_BYTES / 3);
        let big: Vec<StoredEvent> = (0..3).map(|seq| stored(&big_message, seq)).collect();
        let borrowed: Vec<&StoredEvent> = big.iter().collect();
        let chunks = record_sized_chunks(&borrowed);
        assert_eq!(
            chunks.len(),
            2,
            "case large: three third-of-a-record events need two runs, got {} runs",
            chunks.len()
        );
        assert_eq!(
            chunks.iter().map(|chunk| chunk.len()).sum::<usize>(),
            3,
            "case large: every event lands in exactly one run, got {chunks:?}"
        );
    }
}
