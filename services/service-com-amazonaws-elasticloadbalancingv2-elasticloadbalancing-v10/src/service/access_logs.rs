// SPDX-License-Identifier: CC0-1.0

//! The ACCESS LOG a load balancer delivers for a request it answered: the record, the object
//! it is written as, and the bucket the load balancer's own attributes send it to.
//!
//! `LoadBalancerAttribute$Key` carries the destination — "`access_logs.s3.enabled` -
//! Indicates whether access logs are enabled. The value is true or false. The default is
//! false", "`access_logs.s3.bucket` - The name of the S3 bucket for the access logs" and
//! "`access_logs.s3.prefix` - The prefix for the location in the S3 bucket for the access
//! logs" — so WHETHER a request is recorded, and WHERE, is what the load balancer's own
//! attribute collection says, read through the one catalog of those attributes
//! ([`crate::state::load_balancer_attribute`]) so what `DescribeLoadBalancerAttributes`
//! reports and what is delivered cannot drift. A load balancer carrying the attribute at its
//! documented default delivers nothing at all.
//!
//! # Why the record exists
//!
//! The answer a client receives says WHAT the load balancer did; the access log says WHY.
//! The two are the same event seen from either side, and the second is the only place a
//! load-balancer-generated status carries its REASON: a request a load balancer answered
//! itself names the failure class in the record's `error_reason` field, which for a Lambda
//! target is the class of that invocation's failure — the invoke the function's own resource
//! policy admitted no load balancer to make, a run that ended in an unhandled exception, a
//! returned object the response contract refuses, a response past the size such a target may
//! answer with, and a request body past the size one invocation carries. Nothing of a
//! request this plane did not observe is written: a field this emulator has no value for
//! carries the absent marker ([`ABSENT`]) that Elastic Load Balancing writes for one, never
//! an invented value.
//!
//! # What is delivered, and where
//!
//! One object per answered request, gzipped, under the documented layout:
//! `<prefix>/AWSLogs/<account-id>/elasticloadbalancing/<region>/<yyyy>/<mm>/<dd>/` and a file
//! name carrying the account, the service, the Region, the load balancer's own resource id
//! (its forward slashes written as periods), the minute the record belongs to, the address
//! of the node that answered and a unique field that tells two files of one minute apart.
//! Amazon S3 is reached the way any emulated service reaches another — the in-process
//! callback, with this tree's one construction of the object-store client
//! ([`::service_common::s3_client`]) — so no connection is opened for it.
//!
//! Delivery is BEST EFFORT, as the sibling log this plane delivers is
//! ([`super::data_plane`]): an emulator with no object store mounted, or a bucket a client
//! has since deleted, has nowhere to deliver, and no client is failed because a record could
//! not be written.

use std::io::Write as _;
use std::time::{Duration, SystemTime};

use ::smithy_server_runtime::OperationContext;

use crate::state::LoadBalancerRecord;

/// The three attributes that decide whether a request is recorded and where, from
/// `LoadBalancerAttribute$Key`'s own catalog.
const ENABLED_ATTRIBUTE: &str = "access_logs.s3.enabled";
const BUCKET_ATTRIBUTE: &str = "access_logs.s3.bucket";
const PREFIX_ATTRIBUTE: &str = "access_logs.s3.prefix";

/// The first segment of every access-log key, under whatever prefix the attribute names.
const LOG_FILE_ROOT_SEGMENT: &str = "AWSLogs";

/// The segment naming the SERVICE whose files a prefix holds, which is also the second field
/// of a log file's own name: Elastic Load Balancing's `arnNamespace`/`endpointPrefix`.
const LOG_FILE_SERVICE_SEGMENT: &str = "elasticloadbalancing";

/// What an access-log file's name ends with: the log, gzipped.
const LOG_FILE_SUFFIX: &str = ".log.gz";

/// The media type of the object a log file is: a gzip stream.
const GZIP_MEDIA_TYPE: &str = "application/x-gzip";

/// What a field this emulator has no value for carries, which is what Elastic Load Balancing
/// writes for a field that does not apply to a record.
const ABSENT: &str = "-";

/// What a timing field carries where the load balancer never dispatched the request to a
/// target: "-1" is the value Elastic Load Balancing writes for a processing time it could
/// not measure.
const UNMEASURED_TIME: &str = "-1";

/// How many characters of the unique field a log file's name ends with — the field that tells
/// two files of the same minute apart.
const UNIQUE_FIELD_LEN: usize = 16;

/// The segment of a load balancer's ARN its resource id follows:
/// `…:loadbalancer/app/<name>/<id>`.
const ARN_RESOURCE_SEGMENT: &str = "loadbalancer/";

/// The error reasons this plane names for a request it answered ITSELF, each the class
/// Elastic Load Balancing's own `error_reason` field carries for that outcome of a Lambda
/// target.
pub(super) mod reason {
    /// The function's resource policy admits no load balancer to make the invocation, so
    /// none was made.
    pub(crate) const LAMBDA_ACCESS_DENIED: &str = "LambdaAccessDenied";
    /// The run ended in an unhandled exception.
    pub(crate) const LAMBDA_UNHANDLED: &str = "LambdaUnhandled";
    /// The object the function returned is not a response the contract admits.
    pub(crate) const LAMBDA_INVALID_RESPONSE: &str = "LambdaInvalidResponse";
    /// The response is past the size a Lambda target may answer with.
    pub(crate) const LAMBDA_RESPONSE_TOO_LARGE: &str = "LambdaResponseTooLarge";
    /// The request body is past the size one invocation of a Lambda target carries, so the
    /// request never became an event.
    pub(crate) const LAMBDA_REQUEST_TOO_LARGE: &str = "LambdaRequestTooLarge";
}

/// Where one load balancer's access logs go: the bucket its attributes name, the prefix
/// ahead of the layout, and the Region the object store is addressed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Destination {
    pub(super) bucket: String,
    /// The prefix the attribute names, or `None` for a load balancer whose attribute carries
    /// none — whose records land at the bucket's root.
    pub(super) prefix: Option<String>,
    pub(super) region: String,
    /// The account whose load balancer this is, read off its ARN: the account the layout
    /// names, not whoever asked.
    pub(super) account_id: String,
}

/// Where `record` delivers its access logs, or `None` for a load balancer that delivers
/// none: logging switched off (the attribute's documented default), no bucket named, or an
/// ARN that names no account.
pub(super) fn destination(record: &LoadBalancerRecord) -> Option<Destination> {
    if !attribute(record, ENABLED_ATTRIBUTE)
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
    {
        return None;
    }
    let bucket = attribute(record, BUCKET_ATTRIBUTE)?;
    let bucket = bucket.trim().to_string();
    if bucket.is_empty() {
        return None;
    }
    let prefix = attribute(record, PREFIX_ATTRIBUTE)
        .map(|prefix| prefix.trim().trim_matches('/').to_string())
        .filter(|prefix| !prefix.is_empty());
    let account_id = ::service_common::arn::parse(&record.arn)?.account.to_string();
    Some(Destination {
        bucket,
        prefix,
        region: record.region.clone(),
        account_id,
    })
}

/// What `key` holds for `record`: a caller's own write, else the value the attribute is
/// carried at — read through the ONE catalog of load balancer attributes. `None` for a load
/// balancer of a family that carries no such attribute.
fn attribute(record: &LoadBalancerRecord, key: &str) -> Option<String> {
    let spec = crate::state::load_balancer_attribute(key)?;
    if !spec.applies_to(record) {
        return None;
    }
    record
        .attributes
        .get(key)
        .cloned()
        .or_else(|| spec.default_for(record))
}

/// One request a load balancer answered, as its access log records it.
///
/// Every value is read off that request, off the records this service holds about the load
/// balancer and the listener that took it, and off the answer this plane computed — never off
/// another record and never invented.
#[derive(Debug)]
pub(super) struct AnsweredRequest {
    /// The load balancer's own resource id, as the `elb` field carries it: `app/<name>/<id>`.
    pub(super) load_balancer_id: String,
    /// The address of the node that answered — the host of the address the load balancer
    /// publishes, which is where this one answered.
    pub(super) node_address: String,
    /// The protocol of the listener the request reached (`HTTP`/`HTTPS`), which is the
    /// record's `type`.
    pub(super) listener_protocol: Option<String>,
    /// The port of that listener, which the logged request line carries.
    pub(super) listener_port: Option<i32>,
    /// When the load balancer received the request, and when it answered it.
    pub(super) received_at: SystemTime,
    pub(super) answered_at: SystemTime,
    /// How long the TARGET took, where one was called at all.
    pub(super) target_elapsed: Option<Duration>,
    /// The request itself: the method, the host the client addressed, the path and query.
    pub(super) method: String,
    pub(super) host: String,
    pub(super) path: String,
    pub(super) query: String,
    /// The `User-Agent` the request carried, and the `X-Amzn-Trace-Id` it carried.
    pub(super) user_agent: Option<String>,
    pub(super) trace_id: Option<String>,
    /// How many bytes of request body the load balancer received, and how many of response
    /// body it sent.
    pub(super) received_bytes: usize,
    pub(super) sent_bytes: usize,
    /// The status the CLIENT was answered with.
    pub(super) elb_status: u16,
    /// The status the TARGET answered, where a target answered one.
    pub(super) target_status: Option<u16>,
    /// The target group the request was forwarded to, where it was forwarded.
    pub(super) target_group_arn: Option<String>,
    /// The actions the listener performed, in the order they ran.
    pub(super) actions: Vec<String>,
    /// The `Location` a `redirect` action answered with.
    pub(super) redirect_url: Option<String>,
    /// The failure class a load-balancer-generated answer carries ([`reason`]).
    pub(super) error_reason: Option<String>,
    /// The priority of the rule that matched, and 0 where no rule did — which is the value
    /// Elastic Load Balancing writes for a request its default actions answered.
    pub(super) matched_rule_priority: i32,
}

impl Default for AnsweredRequest {
    /// A record with nothing observed: every field at its absent value, and the two instants
    /// at the start of the clock's own epoch — `SystemTime` has no default of its own, and a
    /// record is always built from the instants the request was received and answered at.
    fn default() -> Self {
        Self {
            load_balancer_id: String::new(),
            node_address: String::new(),
            listener_protocol: None,
            listener_port: None,
            received_at: SystemTime::UNIX_EPOCH,
            answered_at: SystemTime::UNIX_EPOCH,
            target_elapsed: None,
            method: String::new(),
            host: String::new(),
            path: String::new(),
            query: String::new(),
            user_agent: None,
            trace_id: None,
            received_bytes: 0,
            sent_bytes: 0,
            elb_status: 0,
            target_status: None,
            target_group_arn: None,
            actions: Vec::new(),
            redirect_url: None,
            error_reason: None,
            matched_rule_priority: 0,
        }
    }
}

impl AnsweredRequest {
    /// The record as one access-log line: the documented fields, in the documented order,
    /// space-separated, with the quoted fields quoted.
    ///
    /// The order is the one Elastic Load Balancing publishes for an HTTP/HTTPS entry — type,
    /// time, elb, client:port, target:port, the three processing times, the two status codes,
    /// the two byte counts, the request, the user agent, the two TLS fields, the target
    /// group, the trace id, the domain name, the chosen certificate, the matched rule's
    /// priority, the request's creation time, the actions, the redirect URL, the error
    /// reason, the target list, the target status list, the two classification fields and the
    /// connection trace id — so a reader that indexes a field by position reads the field it
    /// means.
    pub(super) fn line(&self) -> String {
        let quoted = |value: &str| format!("\"{value}\"");
        let target_address = ABSENT.to_string();
        let fields: Vec<String> = vec![
            self.entry_type(),
            timestamp(self.answered_at),
            self.load_balancer_id.clone(),
            // A request served in-process carries no connection address for this plane to
            // read (see [`super::data_plane`]), so the client's address is not reported.
            ABSENT.to_string(),
            // A Lambda target is reached by invoking it rather than by connecting to an
            // address, which is why Elastic Load Balancing reports none for such an entry.
            target_address.clone(),
            self.request_processing_time(),
            self.target_processing_time(),
            self.response_processing_time(),
            self.elb_status.to_string(),
            self.target_status
                .map_or_else(|| ABSENT.to_string(), |status| status.to_string()),
            self.received_bytes.to_string(),
            self.sent_bytes.to_string(),
            quoted(&self.request_line()),
            quoted(self.user_agent.as_deref().unwrap_or(ABSENT)),
            // The cipher and protocol a TLS connection negotiated are the SOCKET's, which
            // this plane does not observe: it serves the same request however it arrived.
            ABSENT.to_string(),
            ABSENT.to_string(),
            self.target_group_arn
                .clone()
                .unwrap_or_else(|| ABSENT.to_string()),
            quoted(self.trace_id.as_deref().unwrap_or(ABSENT)),
            // The name a TLS client asked for, and the certificate that was presented for
            // it, are the socket's too.
            quoted(ABSENT),
            quoted(ABSENT),
            self.matched_rule_priority.to_string(),
            timestamp(self.received_at),
            quoted(&self.actions.join(",")),
            quoted(self.redirect_url.as_deref().unwrap_or(ABSENT)),
            quoted(self.error_reason.as_deref().unwrap_or(ABSENT)),
            quoted(&target_address),
            quoted(
                &self
                    .target_status
                    .map_or_else(|| ABSENT.to_string(), |status| status.to_string()),
            ),
            // The desync classification of a request is the framing stage's, which this
            // plane does not decide.
            quoted(ABSENT),
            quoted(ABSENT),
            ABSENT.to_string(),
        ];
        fields.join(" ")
    }

    /// The record's `type`: the listener's protocol, lower-cased, and the absent marker for a
    /// request whose listener carried none.
    fn entry_type(&self) -> String {
        self.listener_protocol
            .as_deref()
            .map(str::to_ascii_lowercase)
            .unwrap_or_else(|| ABSENT.to_string())
    }

    /// The request as the record quotes it: the method, the URL the client addressed and the
    /// protocol version. The one socket a load balancer publishes speaks HTTP/1.1
    /// ([`super::listener_socket`]), so that is the version a request arrived under.
    fn request_line(&self) -> String {
        let scheme = self
            .listener_protocol
            .as_deref()
            .map_or("http", |protocol| {
                if protocol.eq_ignore_ascii_case("HTTPS") {
                    "https"
                } else {
                    "http"
                }
            });
        let authority = match self.listener_port {
            Some(port) => format!("{}:{port}", self.host),
            None => self.host.clone(),
        };
        let query = if self.query.is_empty() {
            String::new()
        } else {
            format!("?{}", self.query)
        };
        format!(
            "{} {scheme}://{authority}{}{query} HTTP/1.1",
            self.method, self.path
        )
    }

    /// The time from receiving the request until it was sent to a target, and the absent
    /// timing for a request no target was called for.
    fn request_processing_time(&self) -> String {
        match self.target_elapsed {
            Some(target) => seconds(
                self.answered_at
                    .duration_since(self.received_at)
                    .unwrap_or_default()
                    .saturating_sub(target),
            ),
            None => seconds(
                self.answered_at
                    .duration_since(self.received_at)
                    .unwrap_or_default(),
            ),
        }
    }

    /// The time the target itself took, and the absent timing where none was called.
    fn target_processing_time(&self) -> String {
        self.target_elapsed
            .map_or_else(|| UNMEASURED_TIME.to_string(), seconds)
    }

    /// The time from the target's answer until the client's: nothing of it is spent
    /// elsewhere, so it is reported for a request a target answered and not for one it did
    /// not.
    fn response_processing_time(&self) -> String {
        match self.target_elapsed {
            Some(_) => seconds(Duration::ZERO),
            None => UNMEASURED_TIME.to_string(),
        }
    }
}

/// One access-log file: the record it carries and the object it is written as.
#[derive(Debug)]
pub(super) struct DeliveredRecord {
    pub(super) bucket: String,
    pub(super) key: String,
    pub(super) region: String,
    pub(super) line: String,
}

impl DeliveredRecord {
    /// The object's bytes: the record's line, gzipped — which is what the file IS rather
    /// than a transport encoding, so a reader fetches the object and decompresses it.
    ///
    /// `None` where the encoder itself failed, since no object at all is more honest than
    /// one a reader cannot decompress.
    fn body(&self) -> Option<Vec<u8>> {
        let mut encoder =
            ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
        encoder.write_all(self.line.as_bytes()).ok()?;
        encoder.write_all(b"\n").ok()?;
        encoder.finish().ok()
    }
}

/// Deliver the record of one answered request into the bucket `destination` names.
///
/// Nothing about the answer changes: a failure is traced and the client's own answer stands,
/// for the reason the module docs give.
pub(super) async fn deliver(
    ctx: &OperationContext<'_>,
    destination: &Destination,
    answered: &AnsweredRequest,
) {
    let file = DeliveredRecord {
        bucket: destination.bucket.clone(),
        key: log_file_key(
            destination,
            &answered.load_balancer_id,
            answered.answered_at,
            &answered.node_address,
            &unique_field(),
        ),
        region: destination.region.clone(),
        line: answered.line(),
    };
    let Some(body) = file.body() else {
        ::tracing::warn!(
            bucket = %file.bucket,
            key = %file.key,
            "a load balancer's access-log record could not be framed, so none was delivered"
        );
        return;
    };
    let client = ::service_common::s3_client::s3_client(ctx.callback(), &file.region);
    let answer = client
        .put_object()
        .bucket(&file.bucket)
        .key(&file.key)
        .content_type(GZIP_MEDIA_TYPE)
        .body(::aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await;
    if let Err(error) = answer {
        ::tracing::warn!(
            bucket = %file.bucket,
            key = %file.key,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "a load balancer's access-log record could not be delivered"
        );
    }
}

/// The prefix EVERY access-log file of one load balancer lands under:
/// `<prefix>/AWSLogs/<account-id>/elasticloadbalancing/<region>/`, with a trailing separator.
pub(super) fn log_file_prefix(destination: &Destination) -> String {
    let prefix = destination
        .prefix
        .as_deref()
        .map(|prefix| format!("{prefix}/"))
        .unwrap_or_default();
    format!(
        "{prefix}{LOG_FILE_ROOT_SEGMENT}/{}/{LOG_FILE_SERVICE_SEGMENT}/{}/",
        destination.account_id, destination.region
    )
}

/// The key one access-log file takes: the prefix above, then the DAY the record belongs to,
/// then the file's own name — `<account>_elasticloadbalancing_<region>_<load balancer
/// id>_<YYYYMMDDTHHmm>Z_<node address>_<unique>.log.gz`, with the load balancer's resource id
/// written with periods where its own id carries forward slashes.
pub(super) fn log_file_key(
    destination: &Destination,
    load_balancer_id: &str,
    at: SystemTime,
    node_address: &str,
    unique: &str,
) -> String {
    let at = ::chrono::DateTime::<::chrono::Utc>::from(at);
    let day = at.format("%Y/%m/%d");
    let minute = at.format("%Y%m%dT%H%MZ");
    let named = load_balancer_id.replace('/', ".");
    let account = &destination.account_id;
    let region = &destination.region;
    format!(
        "{}{day}/\
         {account}_{LOG_FILE_SERVICE_SEGMENT}_{region}_{named}_{minute}_{node_address}_\
         {unique}{LOG_FILE_SUFFIX}",
        log_file_prefix(destination)
    )
}

/// The load balancer's own resource id, which the `elb` field of a record carries and the
/// file name is built from: what its ARN names after `loadbalancer/` (`app/<name>/<id>`), and
/// the whole ARN for a value that names none.
pub(super) fn load_balancer_id(arn: &str) -> String {
    arn.split_once(ARN_RESOURCE_SEGMENT)
        .map_or_else(|| arn.to_string(), |(_, id)| id.to_string())
}

/// The unique field one file's name ends with: [`UNIQUE_FIELD_LEN`] hex characters drawn
/// fresh, so two records delivered in the same minute by the same load balancer are two
/// objects and not one overwriting the other.
///
/// Drawn from the maintained generator this crate already draws with, never derived from the
/// request: the field identifies the FILE and nothing about its contents.
fn unique_field() -> String {
    use ::rand::Rng as _;

    let mut rng = ::rand::rng();
    (0..UNIQUE_FIELD_LEN)
        .map(|_| char::from_digit(rng.random_range(0..16), 16).unwrap_or('0'))
        .collect()
}

/// One instant as a record writes it: the ISO 8601 timestamp Elastic Load Balancing records
/// a request's own times in, to microsecond precision.
fn timestamp(at: SystemTime) -> String {
    ::chrono::DateTime::<::chrono::Utc>::from(at)
        .format("%Y-%m-%dT%H:%M:%S%.6fZ")
        .to_string()
}

/// One span as a timing field writes it: seconds with millisecond precision.
fn seconds(elapsed: Duration) -> String {
    format!("{:.3}", elapsed.as_secs_f64())
}

#[cfg(test)]
mod tests {
    //! The layout and the line alone. What a client READS back out of the bucket is pinned
    //! end to end in `tests/all/alb_lambda_target_access_log.rs`.

    use std::io::Read as _;

    use super::*;

    /// An instant with a known UTC calendar rendering: 2026-11-10T21:07:03Z.
    fn at() -> SystemTime {
        ::chrono::DateTime::parse_from_rfc3339("2026-11-10T21:07:03Z")
            .expect("the fixture instant is a well-formed timestamp")
            .into()
    }

    /// The destination the rows below deliver to.
    fn delivering_to(prefix: Option<&str>) -> Destination {
        Destination {
            bucket: "alb-logs".to_string(),
            prefix: prefix.map(str::to_string),
            region: "us-west-2".to_string(),
            account_id: "000000000000".to_string(),
        }
    }

    /// RULE: a file's key is the documented layout — the account, the service and the Region
    /// down to the DAY, and a file name carrying the account, the service, the Region, the
    /// load balancer's resource id with periods for its slashes, the MINUTE, the node's
    /// address and a unique field.
    #[test]
    fn an_access_log_key_carries_the_documented_layout() {
        // The rows span what the layout generalises over: a destination with no prefix, a
        // one-segment prefix and a nested one.
        let rows: &[(&str, Option<&str>, &str)] = &[
            (
                "no prefix",
                None,
                "AWSLogs/000000000000/elasticloadbalancing/us-west-2/2026/11/10/\
                 000000000000_elasticloadbalancing_us-west-2_app.my-lb.0123456789abcdef_\
                 20261110T2107Z_127.0.0.1_abcdef0123456789.log.gz",
            ),
            (
                "a one-segment prefix",
                Some("edge"),
                "edge/AWSLogs/000000000000/elasticloadbalancing/us-west-2/2026/11/10/\
                 000000000000_elasticloadbalancing_us-west-2_app.my-lb.0123456789abcdef_\
                 20261110T2107Z_127.0.0.1_abcdef0123456789.log.gz",
            ),
            (
                "a nested prefix",
                Some("edge/prod"),
                "edge/prod/AWSLogs/000000000000/elasticloadbalancing/us-west-2/2026/11/10/\
                 000000000000_elasticloadbalancing_us-west-2_app.my-lb.0123456789abcdef_\
                 20261110T2107Z_127.0.0.1_abcdef0123456789.log.gz",
            ),
        ];
        for (case, prefix, expected) in rows {
            let key = log_file_key(
                &delivering_to(*prefix),
                "app/my-lb/0123456789abcdef",
                at(),
                "127.0.0.1",
                "abcdef0123456789",
            );
            assert_eq!(
                key, *expected,
                "case {case:?}: expected {expected:?}, got {key:?}"
            );
        }
    }

    /// RULE: a load balancer's resource id is what its ARN names after the resource segment.
    #[test]
    fn a_load_balancer_id_is_the_resource_its_arn_names() {
        let rows: &[(&str, &str, &str)] = &[
            (
                "an application load balancer",
                "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/my-lb/\
                 0123456789abcdef",
                "app/my-lb/0123456789abcdef",
            ),
            (
                "a network load balancer",
                "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/net/my-nlb/\
                 0123456789abcdef",
                "net/my-nlb/0123456789abcdef",
            ),
            ("a value naming no resource", "my-lb", "my-lb"),
        ];
        for (case, arn, expected) in rows {
            let id = load_balancer_id(arn);
            assert_eq!(
                id, *expected,
                "case {case:?}: expected {expected:?}, got {id:?}"
            );
        }
    }

    /// One answered request the line rows below are written from.
    fn answered() -> AnsweredRequest {
        AnsweredRequest {
            load_balancer_id: "app/my-lb/0123456789abcdef".to_string(),
            node_address: "127.0.0.1".to_string(),
            listener_protocol: Some("HTTP".to_string()),
            listener_port: Some(80),
            received_at: at(),
            answered_at: at(),
            target_elapsed: Some(Duration::from_millis(12)),
            method: "GET".to_string(),
            host: "my-lb.example".to_string(),
            path: "/behind/ok".to_string(),
            query: "mode=ok".to_string(),
            user_agent: Some("probe/1.0".to_string()),
            trace_id: Some("Root=1-00000000-000000000000000000000000".to_string()),
            received_bytes: 0,
            sent_bytes: 9,
            elb_status: 200,
            target_status: Some(200),
            target_group_arn: Some(
                "arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/tg/0123456789abcdef"
                    .to_string(),
            ),
            actions: vec!["forward".to_string()],
            matched_rule_priority: 0,
            ..AnsweredRequest::default()
        }
    }

    /// One record line read back the way a reader of the log reads it: the fields are
    /// space-separated, and a quoted field carries its own spaces.
    fn fields(line: &str) -> Vec<String> {
        let mut fields: Vec<String> = Vec::new();
        let mut field = String::new();
        let mut quoted = false;
        for character in line.chars() {
            match character {
                '"' => quoted = !quoted,
                ' ' if !quoted => fields.push(::std::mem::take(&mut field)),
                other => field.push(other),
            }
        }
        fields.push(field);
        fields
    }

    /// One row of the line table: the answer a record is written from, and the fields it
    /// must carry.
    struct LineRow {
        case: &'static str,
        /// What this row changes about the answered request the record is written from.
        change: fn(&mut AnsweredRequest),
        status: u16,
        target_status: &'static str,
        error_reason: &'static str,
    }

    /// RULE: the line carries the documented fields in the documented order, so the field a
    /// reader indexes is the field it means — the error reason among them.
    #[test]
    fn a_record_line_carries_the_documented_fields_in_order() {
        // The rows span what a record generalises over: a request a target answered, and the
        // classes of answer the load balancer generated itself (each with its own reason).
        let rows: &[LineRow] = &[
            LineRow {
                case: "a request the target answered",
                change: |_| {},
                status: 200,
                target_status: "200",
                error_reason: "-",
            },
            LineRow {
                case: "an invocation the function's policy admitted to nobody",
                change: |answered: &mut AnsweredRequest| {
                    answered.elb_status = 502;
                    answered.target_status = None;
                    answered.error_reason = Some(reason::LAMBDA_ACCESS_DENIED.to_string());
                },
                status: 502,
                target_status: "-",
                error_reason: reason::LAMBDA_ACCESS_DENIED,
            },
            LineRow {
                case: "a run that ended in an unhandled exception",
                change: |answered: &mut AnsweredRequest| {
                    answered.elb_status = 502;
                    answered.target_status = None;
                    answered.error_reason = Some(reason::LAMBDA_UNHANDLED.to_string());
                },
                status: 502,
                target_status: "-",
                error_reason: reason::LAMBDA_UNHANDLED,
            },
            LineRow {
                case: "a returned object the contract refuses",
                change: |answered: &mut AnsweredRequest| {
                    answered.elb_status = 502;
                    answered.target_status = None;
                    answered.error_reason = Some(reason::LAMBDA_INVALID_RESPONSE.to_string());
                },
                status: 502,
                target_status: "-",
                error_reason: reason::LAMBDA_INVALID_RESPONSE,
            },
        ];
        for row in rows {
            let LineRow {
                case,
                change,
                status,
                target_status,
                error_reason,
            } = row;
            let mut record = answered();
            change(&mut record);
            let line = record.line();
            let fields = fields(&line);
            assert_eq!(
                fields.len(),
                30,
                "case {case:?}: expected the 30 documented fields, got {} in {line:?}",
                fields.len()
            );
            assert_eq!(
                fields[0], "http",
                "case {case:?}: expected the entry type http, got {:?}",
                fields[0]
            );
            assert_eq!(
                fields[2], "app/my-lb/0123456789abcdef",
                "case {case:?}: expected the load balancer's resource id, got {:?}",
                fields[2]
            );
            assert_eq!(
                fields[6], "0.012",
                "case {case:?}: expected the target's own processing time, got {:?}",
                fields[6]
            );
            assert_eq!(
                fields[8],
                status.to_string(),
                "case {case:?}: expected elb_status_code {status}, got {:?}",
                fields[8]
            );
            assert_eq!(
                fields[9], *target_status,
                "case {case:?}: expected target_status_code {target_status:?}, got {:?}",
                fields[9]
            );
            assert_eq!(
                fields[12], "GET http://my-lb.example:80/behind/ok?mode=ok HTTP/1.1",
                "case {case:?}: expected the request the client sent, got {:?}",
                fields[12]
            );
            assert_eq!(
                fields[22], "forward",
                "case {case:?}: expected the action performed, got {:?}",
                fields[22]
            );
            assert_eq!(
                fields[24], *error_reason,
                "case {case:?}: expected error_reason {error_reason:?} as field 25, got {:?}",
                fields[24]
            );
        }
    }

    /// RULE: the object a record is, is a gzip stream holding that one line.
    #[test]
    fn a_delivered_record_is_a_gzip_stream_of_its_line() {
        let file = DeliveredRecord {
            bucket: "alb-logs".to_string(),
            key: "k".to_string(),
            region: "us-west-2".to_string(),
            line: answered().line(),
        };
        let body = file.body().expect("the record has a body");
        let mut decoded = String::new();
        ::flate2::read::GzDecoder::new(body.as_slice())
            .read_to_string(&mut decoded)
            .unwrap_or_else(|error| panic!("expected a gzip stream, got {error}"));
        assert_eq!(
            decoded.trim_end(),
            answered().line(),
            "expected the record's own line, got {decoded:?}"
        );
    }

    /// One row of the destination table: the attributes a load balancer carries, and the
    /// bucket and prefix they send its records to.
    struct DestinationRow {
        case: &'static str,
        attributes: &'static [(&'static str, &'static str)],
        bucket: Option<&'static str>,
        prefix: Option<&'static str>,
    }

    /// RULE: a load balancer delivers access logs only while its own attributes say so, and
    /// to the bucket and prefix they name.
    #[test]
    fn a_destination_is_what_the_load_balancers_attributes_name() {
        let record = |attributes: &[(&str, &str)]| {
            let mut record = crate::state::LoadBalancerRecord {
                arn: "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/\
                      my-lb/0123456789abcdef"
                    .to_string(),
                id: "0123456789abcdef".to_string(),
                name: "my-lb".to_string(),
                region: "us-west-2".to_string(),
                dns_name: "127.0.0.1:40115".to_string(),
                created_time: at(),
                scheme: "internet-facing".to_string(),
                vpc_id: None,
                state_code: "active".to_string(),
                active_at: at(),
                lb_type: "application".to_string(),
                availability_zones: Vec::new(),
                security_groups: Vec::new(),
                ip_address_type: "ipv4".to_string(),
                customer_owned_ipv4_pool: None,
                enforce_security_group_inbound_rules_on_private_link_traffic: None,
                enable_prefix_for_ipv6_source_nat: None,
                ipv4_ipam_pool_id: None,
                capacity_reservation_modified_at: None,
                network_interface_ids: Vec::new(),
                attributes: ::std::collections::HashMap::new(),
                tags: ::std::collections::HashMap::new(),
                listener_arns: Vec::new(),
            };
            for (key, value) in attributes {
                record
                    .attributes
                    .insert((*key).to_string(), (*value).to_string());
            }
            record
        };
        // The rows span what the reading generalises over: the attribute's documented
        // default, logging on with a bucket and with a prefix, logging on with no bucket
        // named, and logging switched off again with the destination still configured.
        let rows: &[DestinationRow] = &[
            DestinationRow {
                case: "a fresh load balancer",
                attributes: &[],
                bucket: None,
                prefix: None,
            },
            DestinationRow {
                case: "logging on, naming a bucket",
                attributes: &[(ENABLED_ATTRIBUTE, "true"), (BUCKET_ATTRIBUTE, "alb-logs")],
                bucket: Some("alb-logs"),
                prefix: None,
            },
            DestinationRow {
                case: "logging on, naming a bucket and a prefix",
                attributes: &[
                    (ENABLED_ATTRIBUTE, "true"),
                    (BUCKET_ATTRIBUTE, "alb-logs"),
                    (PREFIX_ATTRIBUTE, "/edge/prod/"),
                ],
                bucket: Some("alb-logs"),
                prefix: Some("edge/prod"),
            },
            DestinationRow {
                case: "logging on with no bucket named",
                attributes: &[(ENABLED_ATTRIBUTE, "true")],
                bucket: None,
                prefix: None,
            },
            DestinationRow {
                case: "logging switched off again",
                attributes: &[(ENABLED_ATTRIBUTE, "false"), (BUCKET_ATTRIBUTE, "alb-logs")],
                bucket: None,
                prefix: None,
            },
        ];
        for row in rows {
            let answered = destination(&record(row.attributes));
            let bucket = answered
                .as_ref()
                .map(|destination| destination.bucket.clone());
            let prefix = answered.as_ref().and_then(|destination| destination.prefix.clone());
            assert_eq!(
                bucket.as_deref(),
                row.bucket,
                "case {:?}: expected the bucket {:?}, got {bucket:?}",
                row.case,
                row.bucket
            );
            assert_eq!(
                prefix.as_deref(),
                row.prefix,
                "case {:?}: expected the prefix {:?}, got {prefix:?}",
                row.case,
                row.prefix
            );
        }
    }
}
