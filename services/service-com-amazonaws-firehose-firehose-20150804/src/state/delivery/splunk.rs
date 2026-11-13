// SPDX-License-Identifier: CC0-1.0

//! Delivery to a destination in Splunk: the attempt against the HTTP Event Collector, and
//! the events it did not index.
//!
//! "Describes the configuration of a destination in Splunk"
//! (`SplunkDestinationConfiguration`): the destination is the HEC endpoint
//! `HECEndpoint` names, so the buffer is POSTed there rather than written as an object, and
//! the configuration's required `S3Configuration` is where an event the endpoint never
//! indexed ends up — "Firehose writes any data that could not be indexed to the configured
//! Amazon S3 destination" (`S3BackupMode`), which holds under BOTH of that member's values.
//!
//! What one attempt is made of, each part read from the destination configuration as it
//! stands at the moment of the flush:
//!
//! * WHERE — `HECEndpoint`, with `HECEndpointType` ("either \"Raw\" or \"Event.\"") deciding
//!   what the body of the request carries, and `HECToken` — "a GUID that you obtain from
//!   your Splunk cluster when you create a new HEC endpoint" — carried as the collector's
//!   own authorization scheme.
//! * HOW LONG ONE ATTEMPT MAY TAKE — `HECAcknowledgmentTimeoutInSeconds`, "the amount of
//!   time that Firehose waits to receive an acknowledgment from Splunk after it sends it
//!   data. At the end of the timeout period, Firehose either tries to send the data again or
//!   considers it an error, based on your retry settings."
//! * FOR HOW LONG — `RetryOptions$DurationInSeconds`, "the total amount of time that
//!   Firehose spends on retries. This duration starts after the initial attempt to send data
//!   to Splunk fails", which its own range admits as zero: a destination that is attempted
//!   once and then given up on.
//! * WHERE A FAILED EVENT GOES — the Amazon S3 location `S3Configuration` names, which takes
//!   the per-record error envelope every other destination's failed records are parked as
//!   ([`super::backup_object`]), under an `errorCode` naming the Splunk failure class.
//!
//! The POST is sent through the IN-PROCESS transport of the emulator this delivery runs
//! within ([`super::DeliveryHop`]), exactly as an HTTP endpoint destination's is: the
//! collectors this emulator can deliver to are the ones it serves itself, so an attempt
//! opens no socket and reaches nothing outside this process.

use super::*;

/// What the `firehose:error-output-type` namespace expands to for an event a Splunk
/// destination did not index, and the prefix the backup gets when the configuration names no
/// `ErrorOutputPrefix` — the same `<destination family>-failed` form every other
/// destination's error output carries.
pub(super) const SPLUNK_FAILED_OUTPUT_TYPE: &str = "splunk-failed";

/// The failure of a buffer the collector never answered at all: nothing this emulator serves
/// answers at `HECEndpoint`, or the configuration does not render a request that could be
/// made.
const SPLUNK_CONNECTION_CLOSED: DeliveryFailure = DeliveryFailure {
    output_type: SPLUNK_FAILED_OUTPUT_TYPE,
    error_code: "Splunk.ConnectionClosed",
    error_message: "Unable to send data to Splunk due to a connection failure. This might be \
                    a transient error. Increase the retry duration in your Firehose \
                    configuration.",
};

/// The failure of a buffer no acknowledgment arrived for inside
/// `HECAcknowledgmentTimeoutInSeconds`: the collector did not answer in time, or its answer
/// does not acknowledge the events it was sent.
const SPLUNK_ACK_TIMEOUT: DeliveryFailure = DeliveryFailure {
    output_type: SPLUNK_FAILED_OUTPUT_TYPE,
    error_code: "Splunk.AckTimeout",
    error_message: "Did not receive an acknowledgement from HEC before the HEC \
                    acknowledgement timeout expired.",
};

/// The failure of a buffer the collector refused the token of.
const SPLUNK_INVALID_TOKEN: DeliveryFailure = DeliveryFailure {
    output_type: SPLUNK_FAILED_OUTPUT_TYPE,
    error_code: "Splunk.InvalidToken",
    error_message: "The HEC token is invalid. Update Firehose with a valid HEC token.",
};

/// The failure of a buffer the collector could not read as the events of its endpoint type.
const SPLUNK_INVALID_DATA_FORMAT: DeliveryFailure = DeliveryFailure {
    output_type: SPLUNK_FAILED_OUTPUT_TYPE,
    error_code: "Splunk.InvalidDataFormat",
    error_message: "The data is not formatted correctly. To see how to properly format data \
                    for Raw or Event HEC endpoints, see Splunk Event Data.",
};

/// The failure of a buffer the collector answered an error status for.
const SPLUNK_SERVER_ERROR: DeliveryFailure = DeliveryFailure {
    output_type: SPLUNK_FAILED_OUTPUT_TYPE,
    error_code: "Splunk.ServerError",
    error_message: "Delivery to Splunk failed due to a server error from the Splunk indexer. \
                    Firehose will retry sending the data if the retry duration in your \
                    Firehose is greater than 0.",
};

/// `SplunkBufferingHints$IntervalInSeconds` when the configuration names none: "The default
/// value is 60 (1 minute)."
const DEFAULT_SPLUNK_BUFFER_INTERVAL_IN_SECONDS: i32 = 60;

/// `SplunkBufferingHints$SizeInMBs` when the configuration names none: "The default value is
/// 5."
const DEFAULT_SPLUNK_BUFFER_SIZE_IN_MIB: i32 = 5;

/// `SplunkRetryOptions$DurationInSeconds` when the configuration names none.
///
/// A CHOSEN span, and the SAME one the HTTP endpoint destination takes
/// ([`DEFAULT_ENDPOINT_RETRY_DURATION`]): both members state what the duration MEANS and
/// declare no default, so one decision serves every destination whose buffer is attempted
/// against an endpoint and the two cannot drift apart.
const DEFAULT_SPLUNK_RETRY_DURATION: Duration = DEFAULT_ENDPOINT_RETRY_DURATION;

/// The shortest acknowledgment wait `HECAcknowledgmentTimeoutInSeconds` admits (`min: 180`),
/// and so the wait a configuration naming none gets. A CHOSEN value, since the member
/// declares no default.
const MIN_HEC_ACKNOWLEDGMENT_TIMEOUT_SECONDS: u64 = 180;

/// The longest it admits (`max: 600`), which bounds how long ONE attempt waits for the
/// collector whatever the configuration asks for.
const MAX_HEC_ACKNOWLEDGMENT_TIMEOUT_SECONDS: u64 = 600;

/// `HECAcknowledgmentTimeoutInSeconds` when the configuration names none: the floor of that
/// member's own range.
const DEFAULT_HEC_ACKNOWLEDGMENT_TIMEOUT: Duration =
    Duration::from_secs(MIN_HEC_ACKNOWLEDGMENT_TIMEOUT_SECONDS);

/// The `HECEndpointType` whose endpoint takes the records' own bytes verbatim, rather than
/// one event document per line.
const RAW_ENDPOINT_TYPE: &str = "Raw";

/// How the HEC token is carried: the collector's own authorization scheme, whose credential
/// is the token itself.
const HEC_AUTHORIZATION_SCHEME: &str = "Splunk";

/// The member of the collector's answer stating what it did with the events, whose zero is
/// the acknowledgment ("Success").
const ANSWER_CODE_MEMBER: &str = "code";

/// The `code` an answer that TOOK the events carries.
const ANSWER_SUCCESS_CODE: i64 = 0;

/// One Splunk destination a stream's buffer is delivered to, with everything the attempt
/// needs, read off the destination configuration in force at the moment of the flush.
#[derive(Clone)]
pub struct SplunkTarget {
    /// `HECEndpoint`, the URL the buffer is POSTed to.
    pub endpoint: String,
    /// `HECEndpointType`: which form the request body takes.
    pub endpoint_type: String,
    /// `HECToken`, carried as the collector's authorization. `None` when the configuration
    /// names none or names an empty one (the member's own length range admits zero).
    pub token: Option<String>,
    /// How long one attempt waits for the collector's acknowledgment —
    /// `HECAcknowledgmentTimeoutInSeconds` read inside that member's own range, or
    /// [`DEFAULT_HEC_ACKNOWLEDGMENT_TIMEOUT`].
    pub acknowledgment_timeout: Duration,
    /// `BufferingHints`, which decide when this destination's buffer is due. Carried as the
    /// one shape the window rule reads, with the Splunk defaults filled in, so one rule
    /// serves every destination kind ([`super::delivery_window`]).
    pub hints: Option<BufferingHints>,
    /// How long the delivery of one buffer is retried before its events are parked in
    /// [`Self::backup`] — `RetryOptions$DurationInSeconds`, or
    /// [`DEFAULT_SPLUNK_RETRY_DURATION`].
    pub retry_duration: Duration,
    /// The Amazon S3 location the destination's required `S3Configuration` names, where an
    /// event this collector never indexed is parked.
    pub backup: Option<S3Target>,
}

impl ::std::fmt::Debug for SplunkTarget {
    /// Everything this target carries EXCEPT the collector's own address and credential: the
    /// `HECToken` is "a GUID that you obtain from your Splunk cluster when you create a new
    /// HEC endpoint" (`HECToken`) and so is the caller's secret, and `HECEndpoint` is the
    /// caller's own address — this form says whether one is configured and never what it is,
    /// which the derived form would put in whatever formatted it.
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("SplunkTarget")
            .field("endpoint_type", &self.endpoint_type)
            .field("token", &self.token.is_some())
            .field("acknowledgment_timeout", &self.acknowledgment_timeout)
            .field("hints", &self.hints)
            .field("retry_duration", &self.retry_duration)
            .field("backup", &self.backup)
            .finish_non_exhaustive()
    }
}

/// The Splunk collector a destination configuration delivers to — the endpoint its
/// `HECEndpoint` names — and nothing for a destination of any other kind, nor for one naming
/// no endpoint at all.
#[must_use]
pub fn splunk_targets(config: &DestinationConfigKind) -> Vec<SplunkTarget> {
    let DestinationConfigKind::Splunk(config) = config else {
        return Vec::new();
    };
    if config.hec_endpoint.is_empty() {
        return Vec::new();
    }
    let hints = config.buffering_hints.as_ref();
    vec![SplunkTarget {
        endpoint: config.hec_endpoint.clone(),
        endpoint_type: config.hec_endpoint_type.clone(),
        token: config.hec_token.clone().filter(|token| !token.is_empty()),
        acknowledgment_timeout: config
            .hec_acknowledgment_timeout_in_seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
            // Inside the member's own range, so the wait one attempt can be made to take is
            // the modelled one and never longer.
            .map(|seconds| {
                Duration::from_secs(seconds.clamp(
                    MIN_HEC_ACKNOWLEDGMENT_TIMEOUT_SECONDS,
                    MAX_HEC_ACKNOWLEDGMENT_TIMEOUT_SECONDS,
                ))
            })
            .unwrap_or(DEFAULT_HEC_ACKNOWLEDGMENT_TIMEOUT),
        // Splunk's own buffering defaults, which its `SplunkBufferingHints` members state
        // and which differ from the ones a plain Amazon S3 destination takes: a
        // configuration naming no hints is a destination buffering for the Splunk default,
        // so the window is computed from those values rather than from the generic ones.
        hints: Some(BufferingHints {
            size_in_m_bs: Some(
                hints
                    .and_then(|hints| hints.size_in_m_bs)
                    .unwrap_or(DEFAULT_SPLUNK_BUFFER_SIZE_IN_MIB),
            ),
            interval_in_seconds: Some(
                hints
                    .and_then(|hints| hints.interval_in_seconds)
                    .unwrap_or(DEFAULT_SPLUNK_BUFFER_INTERVAL_IN_SECONDS),
            ),
        }),
        retry_duration: config
            .retry_options
            .as_ref()
            .and_then(|options| options.duration_in_seconds)
            .and_then(|seconds| u64::try_from(seconds).ok())
            .map_or(DEFAULT_SPLUNK_RETRY_DURATION, Duration::from_secs),
        backup: plain_target(&config.s3_configuration),
    }]
}

/// POST one buffer to a Splunk destination's HEC endpoint, answering how the delivery failed
/// when the collector did not take it.
///
/// The request is the one the HTTP Event Collector takes: a `POST` of the buffered records to
/// the configured endpoint, authorized with the HEC token under the collector's own scheme,
/// carrying the records' own bytes — verbatim for a `Raw` endpoint, and one event document
/// per line for an `Event` one, which is how several events are sent in one request. The
/// answer hands the buffer over only when it is a success status whose body states the
/// collector took the events; anything else, and any request that could not be made at all,
/// is a failed delivery whose class the error output names.
///
/// The attempt is bounded by `HECAcknowledgmentTimeoutInSeconds`: an answer that has not
/// arrived by the end of that period is the acknowledgment timeout, which is what that member
/// says happens ("at the end of the timeout period, Firehose either tries to send the data
/// again or considers it an error, based on your retry settings"). The period itself is read
/// inside that member's own range ([`splunk_targets`]), so one attempt's wait is bounded
/// whatever a destination configuration asks for.
pub(super) async fn post_to_collector(
    transport: &SharedHttpConnector,
    delivery: &PendingDelivery,
    splunk: &SplunkTarget,
) -> Result<(), DeliveryFailure> {
    // The body is this buffer's records, so its size is bounded by the per-stream buffer
    // budget (`crate::state::MAX_BUFFERED_BYTES_PER_STREAM`) the put already charged against.
    let raw = splunk.endpoint_type == RAW_ENDPOINT_TYPE;
    let body = collector_body(&delivery.records, raw);
    let mut request = ::http::Request::builder()
        .method(::http::Method::POST)
        .uri(&splunk.endpoint)
        .header(
            ::http::header::CONTENT_TYPE,
            if raw {
                "text/plain"
            } else {
                "application/json"
            },
        );
    if let Some(host) = authority_of(&splunk.endpoint) {
        request = request.header(::http::header::HOST, host);
    }
    if let Some(token) = splunk.token.as_deref() {
        request = request.header(
            ::http::header::AUTHORIZATION,
            format!("{HEC_AUTHORIZATION_SCHEME} {token}"),
        );
    }
    // Every value above comes from the configuration, so an `HECEndpoint` that is not a URI
    // or a token that is not a header value fails HERE rather than panicking.
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
                "the Splunk destination's configuration does not render a delivery request, \
                 so nothing was delivered to it"
            );
            return Err(SPLUNK_CONNECTION_CLOSED);
        }
    };
    let attempt = HttpConnector::call(transport, request);
    let answered = match ::tokio::time::timeout(splunk.acknowledgment_timeout, attempt).await {
        Ok(answered) => answered,
        Err(_) => {
            ::tracing::warn!(
                stream = %delivery.stream,
                timeout = ?splunk.acknowledgment_timeout,
                "the Splunk destination did not acknowledge this delivery inside the \
                 configured acknowledgment timeout"
            );
            return Err(SPLUNK_ACK_TIMEOUT);
        }
    };
    let response = answered.map_err(|error| {
        ::tracing::warn!(
            stream = %delivery.stream,
            error = %error,
            "the Splunk destination could not be reached, so this delivery was left for a \
             later one"
        );
        SPLUNK_CONNECTION_CLOSED
    })?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        ::tracing::warn!(
            stream = %delivery.stream,
            status,
            "the Splunk destination did not take this delivery"
        );
        return Err(refused_with(status));
    }
    if let Err(reason) = acknowledges(&response) {
        ::tracing::warn!(
            stream = %delivery.stream,
            status,
            reason,
            "the Splunk destination's answer does not acknowledge this delivery"
        );
        return Err(SPLUNK_ACK_TIMEOUT);
    }
    Ok(())
}

/// The body of one delivery request: the buffered records' own bytes, in arrival order.
///
/// A `Raw` endpoint "can be either \"Raw\" or \"Event.\"" (`HECEndpointType`) takes the
/// stream's bytes as they were put, so they are concatenated verbatim; an `Event` endpoint
/// takes event documents, which are sent one per line so that a buffer of several events is
/// one request the collector reads a document at a time.
fn collector_body(records: &[StoredRecord], raw: bool) -> Vec<u8> {
    let mut body = Vec::new();
    for record in records {
        if !raw && !body.is_empty() {
            body.extend_from_slice(RECORD_DELIMITER);
        }
        body.extend_from_slice(&record.data);
    }
    body
}

/// How a collector that answered an ERROR status refused this delivery.
///
/// The collector's own classes: a status refusing the credential is the token's
/// ([`SPLUNK_INVALID_TOKEN`]), one refusing the request's content is the data's
/// ([`SPLUNK_INVALID_DATA_FORMAT`]), and anything else is the indexer's
/// ([`SPLUNK_SERVER_ERROR`]) — which is what that code means ("a server error from the Splunk
/// indexer").
fn refused_with(status: u16) -> DeliveryFailure {
    match status {
        401 | 403 => SPLUNK_INVALID_TOKEN,
        400 => SPLUNK_INVALID_DATA_FORMAT,
        _ => SPLUNK_SERVER_ERROR,
    }
}

/// Whether the collector's answer says it TOOK the events, or why it does not.
///
/// The HTTP Event Collector answers one JSON document stating what it did, whose `code` is
/// zero for the events it took. So an answer hands this buffer over only when its body is a
/// JSON object carrying that code; anything else — an unreadable or non-JSON body, a body
/// stating no code, a body stating another one — is a buffer the collector has not
/// acknowledged, which is then retried and finally parked exactly like one it refused
/// outright.
fn acknowledges(
    response: &::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
) -> Result<(), &'static str> {
    let Some(body) = response.body().bytes() else {
        return Err("the answer's body could not be read");
    };
    let Ok(answer) = ::serde_json::from_slice::<::serde_json::Value>(body) else {
        return Err("the answer's body is not JSON");
    };
    let Some(code) = answer
        .get(ANSWER_CODE_MEMBER)
        .and_then(::serde_json::Value::as_i64)
    else {
        return Err("the answer states no code");
    };
    if code != ANSWER_SUCCESS_CODE {
        return Err("the answer states a code of its own");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::types::{
        S3DestinationConfiguration, SplunkBufferingHints, SplunkDestinationConfiguration,
        SplunkRetryOptions,
    };

    /// A destination configuration in Splunk at `endpoint`, with the buffering hints and the
    /// retry options a row configures.
    fn configuration(
        endpoint: &str,
        hints: Option<SplunkBufferingHints>,
        retry_seconds: Option<i32>,
    ) -> DestinationConfigKind {
        DestinationConfigKind::Splunk(Box::new(SplunkDestinationConfiguration {
            hec_endpoint: endpoint.to_string(),
            hec_endpoint_type: "Event".to_string(),
            hec_token: Some("a-token".to_string()),
            buffering_hints: hints,
            retry_options: retry_seconds.map(|seconds| SplunkRetryOptions {
                duration_in_seconds: Some(seconds),
            }),
            s3_configuration: S3DestinationConfiguration {
                role_arn: "arn:aws:iam::000000000000:role/firehose".to_string(),
                bucket_arn: "arn:aws:s3:::splunk-backup".to_string(),
                ..S3DestinationConfiguration::default()
            },
            ..SplunkDestinationConfiguration::default()
        }))
    }

    /// RULE: a configuration naming no buffering hints buffers for SPLUNK's own documented
    /// defaults, and one naming them buffers for what it named.
    #[test]
    fn the_splunk_buffering_defaults_are_the_ones_its_own_hints_document() {
        // (case, the configured hints, the interval and size the target buffers for)
        let rows = [
            (
                "no hints at all",
                None,
                DEFAULT_SPLUNK_BUFFER_INTERVAL_IN_SECONDS,
                DEFAULT_SPLUNK_BUFFER_SIZE_IN_MIB,
            ),
            (
                "the shortest interval the member admits",
                Some(SplunkBufferingHints {
                    interval_in_seconds: Some(0),
                    size_in_m_bs: Some(1),
                }),
                0,
                1,
            ),
            (
                "the longest the members admit",
                Some(SplunkBufferingHints {
                    interval_in_seconds: Some(60),
                    size_in_m_bs: Some(5),
                }),
                60,
                5,
            ),
            (
                "an interval alone",
                Some(SplunkBufferingHints {
                    interval_in_seconds: Some(30),
                    size_in_m_bs: None,
                }),
                30,
                DEFAULT_SPLUNK_BUFFER_SIZE_IN_MIB,
            ),
            (
                "a size alone",
                Some(SplunkBufferingHints {
                    interval_in_seconds: None,
                    size_in_m_bs: Some(2),
                }),
                DEFAULT_SPLUNK_BUFFER_INTERVAL_IN_SECONDS,
                2,
            ),
        ];
        for (case, hints, interval, size) in rows {
            let targets = splunk_targets(&configuration(
                "https://collector.splunk.test/services/collector/event",
                hints,
                None,
            ));
            let read = targets
                .first()
                .unwrap_or_else(|| panic!("case {case:?}: the configuration names a collector"))
                .hints
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: the target carries buffering hints"));
            assert_eq!(
                read.interval_in_seconds,
                Some(interval),
                "case {case:?}: expected an interval of {interval}, got {read:?}"
            );
            assert_eq!(
                read.size_in_m_bs,
                Some(size),
                "case {case:?}: expected a size of {size}, got {read:?}"
            );
        }
    }

    /// RULE: the retry window is `RetryOptions$DurationInSeconds` where the configuration
    /// names one — zero included, which its range admits — and the emulator's own span where
    /// it names none.
    #[test]
    fn the_retry_window_is_the_configured_duration() {
        // (case, the configured seconds, the window the target is retried for)
        let rows = [
            ("no retry options", None, DEFAULT_SPLUNK_RETRY_DURATION),
            ("the range's floor", Some(0), Duration::ZERO),
            ("a minute", Some(60), Duration::from_secs(60)),
            ("the range's ceiling", Some(7_200), Duration::from_secs(7_200)),
        ];
        for (case, configured, expected) in rows {
            let targets = splunk_targets(&configuration(
                "https://collector.splunk.test/services/collector/event",
                None,
                configured,
            ));
            let read = targets
                .first()
                .unwrap_or_else(|| panic!("case {case:?}: the configuration names a collector"))
                .retry_duration;
            assert_eq!(
                read, expected,
                "case {case:?}: expected a retry window of {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: one attempt waits for the collector's acknowledgment for
    /// `HECAcknowledgmentTimeoutInSeconds`, read inside that member's own range (180..600),
    /// and for the range's floor where the configuration names no timeout.
    #[test]
    fn the_acknowledgment_wait_is_the_configured_timeout_inside_its_range() {
        // (case, the configured seconds, the wait one attempt takes). The rows name each
        // published bound of the member's range, a value inside it and one past each end.
        let rows = [
            (
                "no timeout at all",
                None,
                DEFAULT_HEC_ACKNOWLEDGMENT_TIMEOUT,
            ),
            (
                "the range's floor",
                Some(180),
                Duration::from_secs(MIN_HEC_ACKNOWLEDGMENT_TIMEOUT_SECONDS),
            ),
            ("inside the range", Some(300), Duration::from_secs(300)),
            (
                "the range's ceiling",
                Some(600),
                Duration::from_secs(MAX_HEC_ACKNOWLEDGMENT_TIMEOUT_SECONDS),
            ),
            (
                "past the ceiling",
                Some(5_000),
                Duration::from_secs(MAX_HEC_ACKNOWLEDGMENT_TIMEOUT_SECONDS),
            ),
            (
                "below the floor",
                Some(1),
                Duration::from_secs(MIN_HEC_ACKNOWLEDGMENT_TIMEOUT_SECONDS),
            ),
        ];
        for (case, configured, expected) in rows {
            let mut config = configuration(
                "https://collector.splunk.test/services/collector/event",
                None,
                None,
            );
            if let DestinationConfigKind::Splunk(splunk) = &mut config {
                splunk.hec_acknowledgment_timeout_in_seconds = configured;
            }
            let targets = splunk_targets(&config);
            let read = targets
                .first()
                .unwrap_or_else(|| panic!("case {case:?}: the configuration names a collector"))
                .acknowledgment_timeout;
            assert_eq!(
                read, expected,
                "case {case:?}: expected an acknowledgment wait of {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: a destination of another kind, and one naming no HEC endpoint, names no
    /// collector to attempt at all.
    #[test]
    fn only_a_splunk_destination_naming_an_endpoint_is_a_collector() {
        let named = splunk_targets(&configuration(
            "https://collector.splunk.test/services/collector/event",
            None,
            None,
        ));
        assert_eq!(
            named.len(),
            1,
            "case \"an endpoint\": one collector is attempted, got {named:?}"
        );
        let unnamed = splunk_targets(&configuration("", None, None));
        assert!(
            unnamed.is_empty(),
            "case \"no endpoint\": nothing is attempted, got {unnamed:?}"
        );
    }

    /// RULE: the request body carries the records' own bytes — verbatim for a `Raw` endpoint,
    /// and one document per line for an `Event` one.
    #[test]
    fn the_request_body_carries_the_records_own_bytes() {
        let records = |payloads: &[&[u8]]| -> Vec<StoredRecord> {
            payloads
                .iter()
                .enumerate()
                .map(|(index, data)| StoredRecord {
                    record_id: format!("record-{index}"),
                    data: data.to_vec(),
                    received_at: SystemTime::UNIX_EPOCH,
                    delivery_attempts: 0,
                })
                .collect()
        };
        // One row of the body table: how the records are framed for each endpoint type.
        struct BodyRow {
            case: &'static str,
            payloads: &'static [&'static [u8]],
            /// The body a `Raw` endpoint is sent.
            raw: &'static [u8],
            /// The body an `Event` endpoint is sent.
            event: &'static [u8],
        }
        let rows = [
            BodyRow {
                case: "one event",
                payloads: &[b"{\"event\":\"a\"}"],
                raw: b"{\"event\":\"a\"}",
                event: b"{\"event\":\"a\"}",
            },
            BodyRow {
                case: "two events",
                payloads: &[b"{\"event\":\"a\"}", b"{\"event\":\"b\"}"],
                raw: b"{\"event\":\"a\"}{\"event\":\"b\"}",
                event: b"{\"event\":\"a\"}\n{\"event\":\"b\"}",
            },
            BodyRow {
                case: "no records",
                payloads: &[],
                raw: b"",
                event: b"",
            },
            BodyRow {
                case: "bytes that are not text",
                payloads: &[&[0x00, 0xff]],
                raw: &[0x00, 0xff],
                event: &[0x00, 0xff],
            },
        ];
        for row in rows {
            let case = row.case;
            let records = records(row.payloads);
            let raw = collector_body(&records, true);
            assert_eq!(
                raw, row.raw,
                "case {case:?}: a Raw endpoint takes the bytes verbatim, got {raw:?}"
            );
            let event = collector_body(&records, false);
            assert_eq!(
                event, row.event,
                "case {case:?}: an Event endpoint takes one document per line, got {event:?}"
            );
        }
    }

    /// RULE: every failure class a Splunk attempt is parked under names the destination
    /// family, and the status the collector answered decides which one.
    #[test]
    fn every_splunk_failure_names_the_destination_family() {
        // (case, the answered status, the failure class it earns)
        let rows = [
            ("an unauthorized answer", 401, SPLUNK_INVALID_TOKEN),
            ("a forbidden answer", 403, SPLUNK_INVALID_TOKEN),
            ("a refused body", 400, SPLUNK_INVALID_DATA_FORMAT),
            ("a not-found answer", 404, SPLUNK_SERVER_ERROR),
            ("an indexer error", 500, SPLUNK_SERVER_ERROR),
            ("an unavailable indexer", 503, SPLUNK_SERVER_ERROR),
        ];
        for (case, status, expected) in rows {
            let read = refused_with(status);
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
        for failure in [
            SPLUNK_CONNECTION_CLOSED,
            SPLUNK_ACK_TIMEOUT,
            SPLUNK_INVALID_TOKEN,
            SPLUNK_INVALID_DATA_FORMAT,
            SPLUNK_SERVER_ERROR,
        ] {
            assert!(
                failure.error_code.starts_with("Splunk."),
                "case {:?}: every class names the destination family, got {:?}",
                failure.error_code,
                failure.error_code
            );
            assert_eq!(
                failure.output_type, SPLUNK_FAILED_OUTPUT_TYPE,
                "case {:?}: every class is written under the Splunk error output type",
                failure.error_code
            );
        }
    }
}
