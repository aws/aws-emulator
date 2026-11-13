// SPDX-License-Identifier: CC0-1.0

//! The REAL-TIME LOG RECORD the edge delivers for a viewer request it served.
//!
//! A cache behaviour names a real-time log configuration in its `RealtimeLogConfigArn`, and
//! that configuration is what "send[s] real-time log data to the specified Amazon Kinesis
//! data stream" (`CreateRealtimeLogConfig`). So every viewer request the edge serves for a
//! behaviour that names one produces ONE record on each data stream that configuration's
//! `EndPoints` name — as many fields as the configuration includes, tab-delimited, in the
//! order the configuration holds them, which is the order `RealtimeLogConfig$Fields` reports
//! ("in an API response, the fields are provided in the same order in which they are sent to
//! the Amazon Kinesis data stream", [`crate::state::REALTIME_LOG_FIELDS`]). A behaviour that
//! names no configuration produces no record, and so does a request the configuration's
//! `SamplingRate` does not represent
//! ([`crate::state::StoredRealtimeLogConfig::represents_next_request`]).
//!
//! Every value in a record is read off the request the edge served, the answer it served and
//! the distribution's own stored configuration. A field whose value this request HAS NO
//! value for carries [`NO_VALUE`], which is what a real-time log record states for a field
//! with no value: a viewer that sent no `Referer` has no referer, the viewer connection of
//! an in-process request has no TLS parameters and no peer port, and a request no origin
//! answered has no origin latency. Nothing is invented for one.
//!
//! Three readings here are deliberately COARSER than CloudFront's own, because this plane
//! measures one span and serves one answer where CloudFront distinguishes two:
//! `time-to-first-byte` and `time-taken` both report the span the edge spent on the request,
//! `sc-bytes` and `sc-content-len` both report the body the edge served, and
//! `x-edge-detailed-result-type` reports the same classification as `x-edge-result-type`
//! ("the same value as `x-edge-result-type`, except [for] an error").
//!
//! # Where the record goes, and what decides whether it goes at all
//!
//! The write is a `PutRecord` to the stream the endpoint's `KinesisStreamConfig$StreamARN`
//! names, made over the emulator's OWN in-process transport
//! ([`service_common::kinesis_client`]): no socket is opened and the only Kinesis that can
//! answer it is the one this emulator hosts, so a stream ARN a caller submitted never becomes
//! a destination off this machine. The Region the hop is addressed in is read from that ARN
//! and held to the grammar a Region label is written in
//! ([`service_common::arn::region_label`]) before it reaches an endpoint; an ARN that names
//! no Region of a Kinesis stream addresses no stream, and nothing is written for it.
//!
//! The write is made UNDER the role the endpoint names — `KinesisStreamConfig$RoleARN` is
//! "the ARN of an IAM role that CloudFront can use to send real-time log data to your Kinesis
//! data stream" — so what that role is allowed to do on that stream decides whether the
//! record is written at all: a role IAM answers does not allow `kinesis:PutRecord` on the
//! stream delivers NOTHING, while every other endpoint of the same configuration still
//! receives its record. That decision is IAM's, asked of IAM over the same in-process hop
//! ([`service_common::iam_role::role_withholds_action`]), which is also the one route the
//! CloudWatch Logs subscription delivery takes to it.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_runtime_api::http::Request as RtRequest;
use percent_encoding::{AsciiSet, utf8_percent_encode};
use smithy_server_runtime::OperationContext;

use super::generated_headers;
use crate::ops::types::EndPoint;
use crate::service::edge_metrics::ServedFrom;
use crate::state::{ComAmazonawsCloudfrontCloudfront20200531State, KINESIS_STREAM_TYPE};

/// What a record states for a field this request has no value for.
///
/// The marker a CloudFront log record carries in place of a value, for a field that has none
/// on the request it describes.
pub const NO_VALUE: &str = "-";

/// The delimiter between the fields of one record: a real-time log record is tab-delimited.
const FIELD_DELIMITER: char = '\t';

/// The characters a record's FRAMING owns, so no field value may carry one.
///
/// One record is one tab-delimited line, so a tab inside a value would read as another field
/// and a line break as another record. The set is every C0 control character — the tab and
/// the two line-break bytes among them, named here so the set cannot lose them — and nothing
/// else is encoded: the value a field reports is otherwise the one the request, the answer or
/// the configuration carries.
const FRAMING: &AsciiSet = &percent_encoding::CONTROLS
    .add(b'\t')
    .add(b'\r')
    .add(b'\n');

/// The action a `PutRecord` to a data stream is authorized by, which is the one the delivery
/// role has to allow on the stream for a record to be written to it.
const PUT_RECORD_ACTION: &str = "kinesis:PutRecord";

/// The vendor of a Kinesis data stream's ARN, which is where the Region the delivery hop is
/// addressed in is read from.
const KINESIS_VENDOR: &str = "kinesis";

/// How the edge classified the answer it served, as `x-edge-result-type` and
/// `x-edge-response-result-type` report it.
///
/// "How CloudFront classified the response": an answer the edge cache held is a `Hit`, one an
/// origin produced for this request is a `Miss`, a request CloudFront answered itself with an
/// error page is an `Error`, and one it answered with a redirect of its own is a `Redirect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultType {
    /// The edge cache held the answer.
    Hit,
    /// The origin produced the answer for this request.
    Miss,
    /// CloudFront answered the request itself, with an error.
    Error,
    /// CloudFront answered the request itself, with a redirect.
    Redirect,
}

impl ResultType {
    /// How the edge classified the answer it served: who produced it, and whether it is an
    /// error.
    ///
    /// "Error — usually means the HTTP status code is 4xx or 5xx": an answer of either error
    /// class is classified as an error whoever produced it, so an origin's own 404 is an
    /// error rather than a miss. Of the answers that are not errors, one CloudFront produced
    /// ITSELF in the 3xx class is the redirect it answers a `redirect-to-https` viewer with,
    /// one the edge cache held is a hit, and one an origin produced for this request is a
    /// miss; every other answer CloudFront produced in place of the origin's is the edge
    /// answering instead of serving the content, which is its error classification.
    pub fn of_answer(served_from: ServedFrom, status: u16) -> Self {
        if status >= CLIENT_ERROR_STATUS {
            return Self::Error;
        }
        match served_from {
            ServedFrom::EdgeCache => Self::Hit,
            ServedFrom::Origin => Self::Miss,
            ServedFrom::CloudFront
                if (REDIRECTION_STATUS..CLIENT_ERROR_STATUS).contains(&status) =>
            {
                Self::Redirect
            }
            ServedFrom::CloudFront => Self::Error,
        }
    }

    /// The token a record carries this classification as.
    pub fn token(self) -> &'static str {
        match self {
            Self::Hit => "Hit",
            Self::Miss => "Miss",
            Self::Error => "Error",
            Self::Redirect => "Redirect",
        }
    }
}

/// The lowest status of the 3xx class, which is the class the redirect CloudFront answers a
/// `redirect-to-https` viewer with is in, and the lowest of the 4xx class, the first of the
/// two classes an answer is classified as an error by ([`ResultType::of_answer`]).
const REDIRECTION_STATUS: u16 = 300;
const CLIENT_ERROR_STATUS: u16 = 400;

/// One Kinesis data stream a configuration sends its records to, and the role the write is
/// made under.
#[derive(Debug, Clone)]
pub struct KinesisEndPoint {
    /// `KinesisStreamConfig$RoleARN`.
    pub role_arn: String,
    /// `KinesisStreamConfig$StreamARN`.
    pub stream_arn: String,
}

/// The real-time log configuration one viewer request's record goes to, as the store holds
/// it, resolved while the store's lock is held so the delivery itself needs none.
#[derive(Debug, Clone)]
pub struct Configured {
    /// The configuration's own ARN, which is what the behaviour named it by.
    pub arn: String,
    /// The fields each record includes, in the order the configuration holds them.
    pub fields: Vec<String>,
    /// The data streams each record is written to.
    pub end_points: Vec<KinesisEndPoint>,
}

/// The configuration the matched cache behaviour names, WHEN the request being served is one
/// its log data represents; `None` when no record is delivered for this request.
///
/// `None` for each of: a behaviour that names no real-time log configuration, an ARN the
/// store holds no configuration for (a configuration deleted since the behaviour was
/// written), a request the configuration's `SamplingRate` does not represent, and a
/// configuration whose endpoints name no Kinesis data stream to write to.
///
/// Advances the configuration's sampling cursor, which is why it takes the store mutably:
/// which of the covered requests are represented is decided over the SEQUENCE of them, so
/// serving one request is what moves it. A configuration with no data stream to write to
/// covers nothing, so it is resolved BEFORE the cursor moves and spends no part of it.
pub fn sampled(
    store: &mut ComAmazonawsCloudfrontCloudfront20200531State,
    realtime_log_config_arn: Option<&str>,
) -> Option<Configured> {
    let arn = realtime_log_config_arn.filter(|arn| !arn.is_empty())?;
    let stored = store.realtime_log_config_by_arn_mut(arn)?;
    let end_points = kinesis_end_points(&stored.end_points);
    if end_points.is_empty() {
        return None;
    }
    if !stored.represents_next_request() {
        return None;
    }
    Some(Configured {
        arn: stored.arn.clone(),
        fields: stored.fields.clone(),
        end_points,
    })
}

/// The data streams `end_points` name: an endpoint of the one stream type a configuration may
/// carry ([`KINESIS_STREAM_TYPE`]) that names the stream configuration records are sent
/// through.
fn kinesis_end_points(end_points: &[EndPoint]) -> Vec<KinesisEndPoint> {
    end_points
        .iter()
        .filter(|end_point| end_point.stream_type == KINESIS_STREAM_TYPE)
        .filter_map(|end_point| end_point.kinesis_stream_config.as_ref())
        .map(|config| KinesisEndPoint {
            role_arn: config.role_arn.clone(),
            stream_arn: config.stream_arn.clone(),
        })
        .collect()
}

/// One viewer request the edge served, and the answer it served, as the fields of a record
/// report them.
///
/// Everything here is read from the request, from the answer or from the distribution's
/// stored configuration; nothing is derived from the configuration whose record it fills.
#[derive(Debug)]
pub struct LoggedRequest<'a> {
    /// The viewer request itself, which the request-side fields are read off.
    pub req: &'a RtRequest,
    /// When the edge finished responding to the request.
    pub at: SystemTime,
    /// How long the edge had been working on the request when it answered.
    pub taken: Duration,
    /// How long the ORIGIN took to answer, for a request an origin answered.
    pub origin_latency: Option<Duration>,
    /// The status the edge answered.
    pub status: u16,
    /// The bytes of the body the edge served.
    pub response_bytes: usize,
    /// The `Content-Type` the answer carries, when it carries one.
    pub content_type: Option<&'a str>,
    /// The `DomainName` of the distribution that served the request.
    pub distribution_domain: &'a str,
    /// The path within the distribution's endpoint the viewer asked for.
    pub viewer_path: &'a str,
    /// The viewer's query string, without its leading `?`.
    pub query: &'a str,
    /// The country the edge resolved for the viewer.
    pub country: &'a str,
    /// The identifier the edge stamped on the request (`X-Amz-Cf-Id`).
    pub request_id: &'a str,
    /// The `PathPattern` of the cache behaviour that matched, or `None` for the default cache
    /// behaviour, which matches by being last.
    pub path_pattern: Option<&'a str>,
    /// How the edge classified the answer.
    pub result_type: ResultType,
}

impl LoggedRequest<'_> {
    /// The record this request produces for a configuration holding `fields`: one value per
    /// field, in the configuration's own order, tab-delimited.
    pub fn record(&self, fields: &[String]) -> String {
        let mut record = String::new();
        for (position, field) in fields.iter().enumerate() {
            if position > 0 {
                record.push(FIELD_DELIMITER);
            }
            record.push_str(&encoded(&self.value(field)));
        }
        record
    }

    /// What this request states for ONE field of the catalog, and [`NO_VALUE`] for a field it
    /// has no value for — including a field name the catalog does not carry, which names
    /// nothing about a request at all.
    ///
    /// Each arm is the field's own definition from the real-time log configuration fields
    /// documentation ([`crate::state::REALTIME_LOG_FIELDS`] names its source).
    fn value(&self, field: &str) -> String {
        match field {
            // "The date and time at which the edge server finished responding to the
            // request", as seconds on the Unix clock.
            "timestamp" => epoch_seconds(self.at),
            // "The IP address of the viewer that made the request."
            "c-ip" => self.client_ip().to_string(),
            // "The number of seconds between receiving the request and writing the first
            // byte of the response", and the whole span for an answer with no separate first
            // byte to time.
            "time-to-first-byte" | "time-taken" => seconds(self.taken),
            // "The HTTP status code of the response."
            "sc-status" => self.status.to_string(),
            // "The total number of bytes that CloudFront served to the viewer in response to
            // the request", which is the body it served.
            "sc-bytes" | "sc-content-len" => self.response_bytes.to_string(),
            // "The HTTP method of the viewer request."
            "cs-method" => self.req.method().to_ascii_uppercase(),
            // "The protocol of the viewer request", which is the scheme the viewer dialled.
            "cs-protocol" => generated_headers::viewer_scheme(self.req.uri()).to_string(),
            // "The domain name of the CloudFront distribution."
            "cs-host" => self.distribution_domain.to_string(),
            // "The portion of the URI that identifies the path and object."
            "cs-uri-stem" => self.viewer_path.to_string(),
            // "The number of bytes of data that the viewer included in the request,
            // including headers."
            "cs-bytes" => viewer_request_bytes(self.req).to_string(),
            // "An encrypted string that uniquely identifies a request."
            "x-edge-request-id" => self.request_id.to_string(),
            // "The value of the Host header in the viewer request."
            "x-host-header" => self.header("host"),
            // "The IP version of the request", as the viewer's own address states it.
            "c-ip-version" => ip_version(self.client_ip()),
            // The viewer's own headers, each reported as the viewer sent it.
            "cs-user-agent" => self.header("user-agent"),
            "cs-referer" => self.header("referer"),
            "cs-cookie" => self.header("cookie"),
            "x-forwarded-for" => self.header("x-forwarded-for"),
            "cs-accept-encoding" => self.header("accept-encoding"),
            "cs-accept" => self.header("accept"),
            // "The query string portion of the request URL, if any", without its `?`.
            "cs-uri-query" => or_no_value(self.query),
            // "How CloudFront classified the response" — the same classification before the
            // response was written and after its last byte left, there being one answer.
            "x-edge-result-type" | "x-edge-response-result-type"
            | "x-edge-detailed-result-type" => self.result_type.token().to_string(),
            // "The value of the Content-Type header of the response."
            "sc-content-type" => self.content_type.map_or_else(no_value, str::to_string),
            // "The country of the viewer's IP address", as a two-letter code.
            "c-country" => self.country.to_string(),
            // "The path pattern that identifies the cache behavior that matched the viewer
            // request." The DEFAULT cache behaviour matches by being last rather than by a
            // pattern of its own, which is the pattern that matches every path.
            "cache-behavior-path-pattern" => self
                .path_pattern
                .unwrap_or(DEFAULT_BEHAVIOR_PATH_PATTERN)
                .to_string(),
            // "The HTTP headers in the request" and "the names of the HTTP headers in the
            // request", each pair or name on its own line, and how many of them there are.
            "headers" => or_no_value(&self.header_lines(|name, value| format!("{name}:{value}"))),
            "header-names" => {
                or_no_value(&self.header_lines(|name, _value| name.to_string()))
            }
            "headers-count" => self.req.headers().iter().count().to_string(),
            // "The total number of seconds that CloudFront spent to receive the first byte /
            // the last byte of the response from the origin", for a request an ORIGIN
            // answered: a request CloudFront answered itself, or the edge cache answered,
            // reached no origin and so has neither.
            "origin-fbl" | "origin-lbl" => {
                self.origin_latency.map_or_else(no_value, seconds)
            }
            // Every other field of the catalog describes something this request HAS NOT: the
            // edge location and the viewer's port and autonomous system, which an in-process
            // viewer connection has none of; the TLS parameters of a connection whose
            // encryption this plane did not negotiate; the field-level encryption of a
            // request no profile encrypted; the range of an answer that is not a range; and
            // the primary distribution of a request not served by a staging one. A name the
            // catalog does not carry describes nothing about a request either.
            _ => no_value(),
        }
    }

    /// The address the request came from, by the ONE reading of it this emulator has: a
    /// request served in process crossed no socket from a remote peer, so the address is the
    /// one its forwarded-for header states, and the loopback address it really arrived from
    /// otherwise.
    fn client_ip(&self) -> &str {
        self.req
            .headers()
            .get(FORWARDED_FOR_HEADER)
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|address| address.parse::<std::net::IpAddr>().is_ok())
            .unwrap_or(IN_PROCESS_CLIENT_IP)
    }

    /// One header of the viewer request, and [`NO_VALUE`] for a header it did not carry.
    fn header(&self, name: &str) -> String {
        self.req
            .headers()
            .get(name)
            .map_or_else(no_value, str::to_string)
    }

    /// The viewer's headers, one `rendered` line each, in the order they arrived.
    fn header_lines(&self, rendered: impl Fn(&str, &str) -> String) -> String {
        self.req
            .headers()
            .iter()
            .map(|(name, value)| rendered(name, value))
            .collect::<Vec<String>>()
            .join("\n")
    }
}

/// The `PathPattern` reported for the DEFAULT cache behaviour, which "CloudFront applies …
/// if none of the path patterns match" — the pattern that matches every path.
const DEFAULT_BEHAVIOR_PATH_PATTERN: &str = "*";

/// The header a viewer that traversed a proxy states its own address in, and the address a
/// request that states none came from — the reading
/// `service_com_amazonaws_wafv2_awswaf_20190729::service::edge::InspectedRequest` takes of
/// the same question for the same requests, so the edge reports one viewer one address
/// whichever of the two asks.
const FORWARDED_FOR_HEADER: &str =
    ::service_com_amazonaws_wafv2_awswaf_20190729::service::edge::FORWARDED_FOR_HEADER;
const IN_PROCESS_CLIENT_IP: &str =
    ::service_com_amazonaws_wafv2_awswaf_20190729::service::edge::IN_PROCESS_CLIENT_IP;

/// Deliver `record` to every data stream `configured` names.
///
/// One `PutRecord` per endpoint, under `partition_key` — the identifier of the request the
/// record describes, so one request's record is one record of the stream and a stream with
/// several shards spreads the requests over them. An endpoint whose role withholds the write,
/// whose ARN names no stream of a Region, or whose stream Kinesis would not take the record
/// for, is left with nothing: the record was not written there, which is exactly what a
/// client polling that stream sees.
pub async fn deliver(
    ctx: &OperationContext<'_>,
    configured: &Configured,
    record: &str,
    partition_key: &str,
) {
    for end_point in &configured.end_points {
        let Some(region) = ::service_common::arn::region_of(&end_point.stream_arn, KINESIS_VENDOR)
            .and_then(::service_common::arn::region_label)
        else {
            ::tracing::debug!(
                realtime_log_config = %configured.arn,
                "this endpoint's StreamARN names no Kinesis data stream of a Region, so no \
                 record was written for it"
            );
            continue;
        };
        // The write is the ROLE's to make, so a role that may not make it writes nothing —
        // and the absence on that stream is the whole of what a client sees.
        if ::service_common::iam_role::role_withholds_action(
            ctx,
            region,
            &end_point.role_arn,
            PUT_RECORD_ACTION,
            &end_point.stream_arn,
        )
        .await
        {
            ::tracing::debug!(
                realtime_log_config = %configured.arn,
                "the role this endpoint names is not allowed to put a record on its data \
                 stream, so no record was written to it"
            );
            continue;
        }
        let written = ::service_common::kinesis_client::kinesis_client(ctx, region)
            .put_record()
            .stream_arn(&end_point.stream_arn)
            .partition_key(partition_key)
            .data(::aws_sdk_kinesis::primitives::Blob::new(
                record.as_bytes().to_vec(),
            ))
            .send()
            .await;
        if let Err(error) = written {
            ::tracing::warn!(
                realtime_log_config = %configured.arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the data stream this endpoint names did not take the real-time log record"
            );
        }
    }
}

/// `value` with the characters a record's framing owns percent-encoded ([`FRAMING`]).
fn encoded(value: &str) -> String {
    utf8_percent_encode(value, FRAMING).to_string()
}

/// [`NO_VALUE`], as a value.
fn no_value() -> String {
    NO_VALUE.to_string()
}

/// `value`, or [`NO_VALUE`] when the request carried nothing in it.
fn or_no_value(value: &str) -> String {
    if value.is_empty() {
        no_value()
    } else {
        value.to_string()
    }
}

/// `at` as seconds on the Unix clock, which is the instant a record's `timestamp` carries.
fn epoch_seconds(at: SystemTime) -> String {
    at.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

/// One measured span, in seconds, as the fields that report a duration carry it: whole
/// seconds and the milliseconds within them.
fn seconds(span: Duration) -> String {
    format!("{:.3}", span.as_secs_f64())
}

/// The IP version of `address`, as `c-ip-version` reports it.
fn ip_version(address: &str) -> String {
    match address.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(_)) => "IPv4".to_string(),
        Ok(std::net::IpAddr::V6(_)) => "IPv6".to_string(),
        Err(_) => no_value(),
    }
}

/// The bytes one viewer request occupies, "including headers": its request line, each header
/// line, the empty line that ends them and the body it carried, as HTTP/1.1 frames them.
fn viewer_request_bytes(req: &RtRequest) -> usize {
    /// `" HTTP/1.1\r\n"` after the request target, and the `\r\n` that ends the headers.
    const REQUEST_LINE_FRAMING: usize = 11;
    const HEADER_FRAMING: usize = 2;
    /// `": "` between a header's name and its value, and the `\r\n` that ends the line.
    const HEADER_LINE_FRAMING: usize = 4;
    let headers: usize = req
        .headers()
        .iter()
        .map(|(name, value)| name.len() + value.len() + HEADER_LINE_FRAMING)
        .sum();
    req.method().len()
        + 1
        + req.uri().len()
        + REQUEST_LINE_FRAMING
        + headers
        + HEADER_FRAMING
        + req.body().bytes().map_or(0, <[u8]>::len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::REALTIME_LOG_FIELDS;
    use aws_smithy_types::body::SdkBody;

    /// One viewer request carrying `headers`, addressed at `uri`.
    fn request(uri: &str, headers: &[(&str, &str)]) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_uri(uri.to_string()).expect("a valid URI");
        for (name, value) in headers {
            req.headers_mut()
                .insert((*name).to_string(), (*value).to_string());
        }
        req
    }

    /// One served request to read field values off.
    fn served<'a>(req: &'a RtRequest, result_type: ResultType) -> LoggedRequest<'a> {
        LoggedRequest {
            req,
            at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            taken: Duration::from_millis(1_500),
            origin_latency: Some(Duration::from_millis(500)),
            status: 200,
            response_bytes: 42,
            content_type: Some("text/plain"),
            distribution_domain: "d111111abcdef8.cloudfront.net",
            viewer_path: "/probe.txt",
            query: "a=1",
            country: "US",
            request_id: "0123456789abcdef0123456789abcdef",
            path_pattern: None,
            result_type,
        }
    }

    #[test]
    fn a_record_carries_one_value_per_field_in_the_configurations_order() {
        let req = request("/probe.txt?a=1", &[("host", "cdn.example.com")]);
        let served = served(&req, ResultType::Miss);
        // Rows differ along the field: one computed from the clock, one off the answer, two
        // off the request, one the distribution's own, and two this request has no value
        // for.
        // (case, the field, the value it carries)
        let rows: [(&str, &str, &str); 8] = [
            ("the instant the edge answered", "timestamp", "1700000000"),
            ("the status", "sc-status", "200"),
            ("the path", "cs-uri-stem", "/probe.txt"),
            ("the classification", "x-edge-response-result-type", "Miss"),
            ("the host the viewer addressed", "x-host-header", "cdn.example.com"),
            (
                "the distribution's own name",
                "cs-host",
                "d111111abcdef8.cloudfront.net",
            ),
            ("a header the viewer did not send", "cs-referer", NO_VALUE),
            (
                "the viewer's port, which an in-process request has none of",
                "c-port",
                NO_VALUE,
            ),
        ];
        for (case, field, expected) in rows {
            let fields = vec![field.to_string()];
            let record = served.record(&fields);
            assert_eq!(
                record, expected,
                "case {case:?} ({field}): expected the record {expected:?}, got {record:?}"
            );
        }
        // The whole record is the field values in the order the configuration holds them,
        // tab-delimited, one value per field and nothing else.
        let fields: Vec<String> = ["sc-status", "cs-uri-stem", "c-country"]
            .iter()
            .map(|field| (*field).to_string())
            .collect();
        let record = served.record(&fields);
        assert_eq!(
            record, "200\t/probe.txt\tUS",
            "expected the three values tab-delimited in the configuration's order, got \
             {record:?}"
        );
    }

    #[test]
    fn a_value_never_carries_the_characters_the_framing_owns() {
        // A viewer header whose value carries a TAB, which HTTP field values admit: left as
        // it is, it would make one record read as one field more than the configuration
        // includes.
        let req = request("/probe.txt", &[("user-agent", "curl\t8")]);
        let served = served(&req, ResultType::Miss);
        let fields = vec!["cs-user-agent".to_string(), "sc-status".to_string()];
        let record = served.record(&fields);
        assert_eq!(
            record, "curl%098\t200",
            "expected the tab inside the value encoded and one delimiter between the two \
             fields, got {record:?}"
        );
        assert_eq!(
            record.matches(FIELD_DELIMITER).count(),
            1,
            "expected one delimiter for two fields, got {record:?}"
        );
        // And a line break, which no field value of a single-line record may carry either.
        assert_eq!(
            encoded("one\r\ntwo"),
            "one%0D%0Atwo",
            "expected a line break inside a value encoded, got {:?}",
            encoded("one\r\ntwo")
        );
    }

    #[test]
    fn an_answer_is_classified_by_who_produced_it_and_whether_it_is_an_error() {
        // Rows differ along who produced the answer and the class of its status.
        // (case, who produced it, the status, the classification)
        let rows: [(&str, ServedFrom, u16, ResultType); 8] = [
            ("the origin's own answer", ServedFrom::Origin, 200, ResultType::Miss),
            (
                "an answer the edge cache held",
                ServedFrom::EdgeCache,
                200,
                ResultType::Hit,
            ),
            (
                "the redirect CloudFront answers a plaintext viewer with",
                ServedFrom::CloudFront,
                301,
                ResultType::Redirect,
            ),
            (
                "a request CloudFront refused itself",
                ServedFrom::CloudFront,
                403,
                ResultType::Error,
            ),
            (
                "an origin CloudFront could not reach",
                ServedFrom::CloudFront,
                502,
                ResultType::Error,
            ),
            (
                "the origin's own not-found",
                ServedFrom::Origin,
                404,
                ResultType::Error,
            ),
            (
                "a not-modified the cache answered",
                ServedFrom::EdgeCache,
                304,
                ResultType::Hit,
            ),
            (
                "an answer CloudFront produced in place of the origin's",
                ServedFrom::CloudFront,
                200,
                ResultType::Error,
            ),
        ];
        for (case, served_from, status, expected) in rows {
            let actual = ResultType::of_answer(served_from, status);
            assert_eq!(
                actual, expected,
                "case {case:?} ({served_from:?}, {status}): expected {expected:?}, got \
                 {actual:?}"
            );
        }
    }

    #[test]
    fn each_classification_has_its_own_token() {
        let cases = [
            ("the edge cache answered", ResultType::Hit, "Hit"),
            ("the origin answered", ResultType::Miss, "Miss"),
            ("CloudFront refused it", ResultType::Error, "Error"),
            ("CloudFront redirected it", ResultType::Redirect, "Redirect"),
        ];
        for (case, result_type, expected) in cases {
            let actual = result_type.token();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the token {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn every_field_of_the_catalog_has_a_value() {
        let req = request("/probe.txt?a=1", &[("host", "cdn.example.com")]);
        let served = served(&req, ResultType::Miss);
        for field in REALTIME_LOG_FIELDS {
            let value = served.value(field);
            assert!(
                !value.is_empty(),
                "case {field:?}: every field of the catalog carries a value or the no-value \
                 marker, got {value:?}"
            );
        }
    }
}
