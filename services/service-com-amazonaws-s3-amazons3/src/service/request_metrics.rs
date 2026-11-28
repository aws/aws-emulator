// SPDX-License-Identifier: CC0-1.0

//! The `AWS/S3` REQUEST metrics this service publishes about the requests it serves on a
//! bucket whose client asked for them.
//!
//! # What a request metric is
//!
//! S3 publishes request metrics for a bucket only where a client has asked for them, by
//! storing a metrics configuration on it (`PutBucketMetricsConfiguration`: "Sets a metrics
//! configuration (specified by the metrics configuration ID) for the bucket"). Each
//! configuration is its OWN series: the datapoints are dimensioned by the bucket the request
//! addressed ([`BUCKET_NAME_DIMENSION`]) and by the id of the configuration the request fell
//! under ([`FILTER_ID_DIMENSION`]) — "this dimension filters metrics configurations that you
//! specify for request metrics on a bucket, for example, a prefix, tag, or access point"
//! (Amazon S3 User Guide, *Metrics and dimensions*) — so a bucket carrying two
//! configurations has two series of every metric below, and a bucket carrying none has no
//! series at all.
//!
//! WHICH configurations one request falls under is each configuration's `Filter`
//! ([`matches`]): a configuration with no filter covers the whole bucket, and a filtered one
//! covers the requests whose addressed object the filter selects.
//!
//! # What each request contributes
//!
//! Every request counted contributes ONE observation to each series below, which is what
//! makes a period that had requests carry a datapoint of every one of them rather than only
//! of the ones that happened to be non-zero (Amazon S3 User Guide, *Amazon S3 CloudWatch
//! request metrics*, from which each definition below is quoted):
//!
//! * [`ALL_REQUESTS`] — "the total number of HTTP requests made to an Amazon S3 bucket,
//!   regardless of type": one per request;
//! * the counter of the request's own TYPE — [`GET_REQUESTS`] ("the number of HTTP GET
//!   requests made for objects in an Amazon S3 bucket. This doesn't include list
//!   operations"), [`PUT_REQUESTS`], [`DELETE_REQUESTS`], [`HEAD_REQUESTS`],
//!   [`POST_REQUESTS`] or [`LIST_REQUESTS`] ("the number of HTTP requests that list the
//!   contents of a bucket") — one per request of that type, which is what
//!   [`RequestKind::counter`] names;
//! * [`FOUR_XX_ERRORS`] and [`FIVE_XX_ERRORS`] — "the number of HTTP 4xx client error status
//!   code requests made to an Amazon S3 bucket with a value of either 0 or 1", and the same
//!   for the 5xx class: one observation per request, 1 when that request's own answer carried
//!   a status of the class and 0 when it did not, so the `Sum` over a period is how many
//!   requests of that period failed that way and the `Average` is the rate. A request that
//!   was SERVED therefore publishes a zero-valued error datapoint, which is what tells a
//!   client watching a quiet prefix from one watching a bucket that reports nothing;
//! * [`BYTES_DOWNLOADED`] — "the number of bytes downloaded for requests made to an Amazon S3
//!   bucket, where the response includes a body" — and [`BYTES_UPLOADED`] — "the number of
//!   bytes uploaded that contain a request body, made to an Amazon S3 bucket": observed only
//!   for a request that carried a body in that direction, because that is the request each
//!   definition counts.
//! * [`FIRST_BYTE_LATENCY`] — "the per-request time from the complete request being received by
//!   an Amazon S3 bucket to when the response starts to be returned" — and
//!   [`TOTAL_REQUEST_LATENCY`] — "the elapsed per-request time from the first byte received to
//!   the last byte sent to an Amazon S3 bucket", which "includes the time taken to receive the
//!   request body and send the response body": one observation each, in `Milliseconds`, of the
//!   spans [`RequestClock`] MEASURED while this service served that one request. Every request
//!   takes time to serve, so every counted request observes both, which is what makes a
//!   client's `Average`/`Maximum` over a period the latency of the requests that period really
//!   carried.
//!
//! # Where the datapoints go
//!
//! Into the CloudWatch emulator's OWN metric store — the same store `ListMetrics`,
//! `GetMetricStatistics` and the alarm evaluation read — reached through the shared engine, so
//! one metric has one home and the publisher and its readers cannot drift. They never travel
//! over `PutMetricData`, which refuses a publish into a namespace the services own ("you
//! should not specify a namespace that begins with `AWS/`", `PutMetricDataInput$Namespace`).
//!
//! # How a request reaches here
//!
//! [`Metered`] wraps the operation: the request is served exactly as before and the answer is
//! counted afterwards, so the status, the response body and the request body counted are the
//! ones the caller really got and sent — a refusal the framework's own walk made included.
//! [`serve_metered`] is the table of which operations are wrapped.
//!
//! What that leaves out is a request the framework refused BEFORE it named an operation — one
//! whose signature the emulator did not verify, or whose address names no operation at all:
//! those are answered ahead of any operation, so no operation of this service sees them and
//! none counts them.
//!
//! What one request's fold costs is bounded by the bucket's own configuration count, which
//! this service's store caps at
//! [`crate::state::MAX_METRICS_CONFIGURATIONS_PER_BUCKET`] — the published "up to 1,000
//! metrics configurations per bucket" — times the fixed number of series above; and each
//! append is bounded in turn by the metric store's own per-series and series-count caps.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;
use ::service_common::front_door::FrontDoor;
use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Service,
};

use crate::ops::types::{MetricsFilter, Tag};
use crate::state::ResourceTag;

/// The namespace S3 reports its own metrics in.
pub const NAMESPACE: &str = "AWS/S3";

/// The dimension naming the bucket a datapoint is about.
pub const BUCKET_NAME_DIMENSION: &str = "BucketName";

/// The dimension naming the metrics configuration a datapoint was counted under.
pub const FILTER_ID_DIMENSION: &str = "FilterId";

/// How many requests of every type the bucket served.
pub const ALL_REQUESTS: &str = "AllRequests";

/// How many `GET` requests of an object the bucket served.
pub const GET_REQUESTS: &str = "GetRequests";

/// How many `PUT` requests of an object the bucket served.
pub const PUT_REQUESTS: &str = "PutRequests";

/// How many `DELETE` requests of an object the bucket served.
pub const DELETE_REQUESTS: &str = "DeleteRequests";

/// How many `HEAD` requests the bucket served.
pub const HEAD_REQUESTS: &str = "HeadRequests";

/// How many `POST` requests the bucket served.
pub const POST_REQUESTS: &str = "PostRequests";

/// How many requests that list the bucket's contents it served.
pub const LIST_REQUESTS: &str = "ListRequests";

/// How many bytes the bucket's answers carried.
pub const BYTES_DOWNLOADED: &str = "BytesDownloaded";

/// How many bytes the requests to the bucket carried.
pub const BYTES_UPLOADED: &str = "BytesUploaded";

/// Whether a request was answered with a 4xx status: 1 when it was, 0 when it was not.
pub const FOUR_XX_ERRORS: &str = "4xxErrors";

/// Whether a request was answered with a 5xx status: 1 when it was, 0 when it was not.
pub const FIVE_XX_ERRORS: &str = "5xxErrors";

/// How long the bucket took to begin answering a request.
pub const FIRST_BYTE_LATENCY: &str = "FirstByteLatency";

/// How long the bucket took to serve a request in full.
pub const TOTAL_REQUEST_LATENCY: &str = "TotalRequestLatency";

/// The unit a counter of requests is published in: `Count`, the `StandardUnit` value for a
/// number of things.
pub const COUNT_UNIT: &str = "Count";

/// The unit the byte counters are published in.
pub const BYTES_UNIT: &str = "Bytes";

/// The unit the two latencies are published in: milliseconds, the granularity S3 reports a
/// request's latency at.
pub const MILLISECONDS_UNIT: &str = "Milliseconds";

/// Milliseconds in a second: the conversion between a measured span and the unit a latency is
/// published in.
const MILLIS_PER_SECOND: f64 = 1_000.0;

/// The value a request contributes to the counter of its own class: it was one request.
const ONE_REQUEST: f64 = 1.0;

/// The value a request contributes to the counter of a class it is not of — "either 0 or 1",
/// and this is the 0.
const NOT_OF_THE_CLASS: f64 = 0.0;

/// The `StorageResolution` these datapoints are published at, in seconds: regular resolution,
/// "which CloudWatch stores at 1-minute resolution" (`MetricDatum$StorageResolution`), the
/// granularity S3 reports a bucket's request metrics at ("available at 1-minute intervals").
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// The lowest status of the 4xx class, and of the 5xx class, and the first status past both.
const CLIENT_ERROR_STATUS: u16 = 400;
const SERVER_ERROR_STATUS: u16 = 500;
const PAST_SERVER_ERROR_STATUS: u16 = 600;

/// Which TYPE of request one served request is, which decides the per-type counter it is
/// counted into.
///
/// A request's type is what its own counter's definition names it by: the HTTP method it was
/// made with, except that a request that lists the bucket's contents is a LIST rather than the
/// `GET` it travels as ([`LIST_REQUESTS`], and "this doesn't include list operations" on
/// [`GET_REQUESTS`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    /// A read of an object.
    Get,
    /// A write of an object, or of a part of one.
    Put,
    /// A delete of an object.
    Delete,
    /// A read of an object's or a bucket's metadata alone.
    Head,
    /// A `POST`: the upload a browser form makes, and the two calls that open and close a
    /// multipart upload.
    Post,
    /// A listing of what the bucket holds.
    List,
}

impl RequestKind {
    /// The per-type counter a request of this type is counted into.
    pub fn counter(self) -> &'static str {
        match self {
            Self::Get => GET_REQUESTS,
            Self::Put => PUT_REQUESTS,
            Self::Delete => DELETE_REQUESTS,
            Self::Head => HEAD_REQUESTS,
            Self::Post => POST_REQUESTS,
            Self::List => LIST_REQUESTS,
        }
    }
}

/// How long serving ONE request took, as the two latency series read it.
///
/// Both spans are MEASURED on a monotonic clock while the request was served ([`RequestClock`]),
/// never derived from what the request asked for: a latency series is a measurement of this
/// server or it is nothing.
#[derive(Debug, Clone, Copy)]
pub struct ServedLatency {
    /// Up to the first byte of the answer: "the per-request time from the complete request being
    /// received by an Amazon S3 bucket to when the response starts to be returned".
    pub to_first_byte: Duration,
    /// The request as a whole: "the elapsed per-request time from the first byte received to the
    /// last byte sent", so never shorter than the span up to the first byte.
    pub total: Duration,
}

/// The clock one request this service is serving is timed on.
///
/// Started where serving that request begins ([`RequestClock::started`]) and marked at the
/// instant its answer's first byte exists ([`RequestClock::first_byte`]); the whole span is read
/// last, once the answer including its body is complete. A request arrives here with its body
/// already received in full and is answered with a buffered body, so both marks are taken around
/// the work this service does for the request: the whole-request span covers the answer up to
/// and including its last byte being in hand, and not what the transport then spends handing
/// that buffer over.
#[derive(Debug, Clone, Copy)]
pub struct RequestClock {
    /// When serving the request began.
    started: Instant,
    /// What had elapsed when its answer's first byte existed.
    to_first_byte: Duration,
}

impl RequestClock {
    /// Start timing a request that is about to be served.
    #[must_use]
    pub fn started() -> Self {
        Self {
            started: Instant::now(),
            to_first_byte: Duration::ZERO,
        }
    }

    /// Mark the instant the answer's first byte exists.
    #[must_use]
    pub fn first_byte(self) -> Self {
        Self {
            to_first_byte: self.started.elapsed(),
            ..self
        }
    }

    /// The two latencies this request publishes, the whole-request span read at this instant:
    /// call it once the answer, body included, is complete.
    #[must_use]
    pub fn latency(self) -> ServedLatency {
        ServedLatency {
            to_first_byte: self.to_first_byte,
            total: self.started.elapsed(),
        }
    }
}

/// ONE request this service served, as these series read it.
#[derive(Debug)]
pub struct ServedRequest {
    /// Which type of request it was.
    pub kind: RequestKind,
    /// The status its answer carried.
    pub status: u16,
    /// How many bytes of body the answer carried, for an answer that carried one.
    pub bytes_downloaded: Option<usize>,
    /// How many bytes of body the request carried, for a request that carried one.
    pub bytes_uploaded: Option<usize>,
    /// How long serving it took.
    pub latency: ServedLatency,
}

impl ServedRequest {
    /// What this one request contributes to each of the bucket's series: the metric, the
    /// value, and the unit the value is published under.
    ///
    /// The four counters every request contributes to come first, then the two latencies every
    /// request also observes, and the two byte counters only for the direction this request
    /// really carried a body in.
    fn observations(&self) -> Vec<(&'static str, f64, &'static str)> {
        let four_xx = (CLIENT_ERROR_STATUS..SERVER_ERROR_STATUS).contains(&self.status);
        let five_xx = (SERVER_ERROR_STATUS..PAST_SERVER_ERROR_STATUS).contains(&self.status);
        let mut observations = vec![
            (ALL_REQUESTS, ONE_REQUEST, COUNT_UNIT),
            (self.kind.counter(), ONE_REQUEST, COUNT_UNIT),
            (FOUR_XX_ERRORS, of_the_class(four_xx), COUNT_UNIT),
            (FIVE_XX_ERRORS, of_the_class(five_xx), COUNT_UNIT),
            (
                FIRST_BYTE_LATENCY,
                as_millis(self.latency.to_first_byte),
                MILLISECONDS_UNIT,
            ),
            (
                TOTAL_REQUEST_LATENCY,
                as_millis(self.latency.total),
                MILLISECONDS_UNIT,
            ),
        ];
        if let Some(bytes) = self.bytes_downloaded {
            observations.push((BYTES_DOWNLOADED, as_bytes(bytes), BYTES_UNIT));
        }
        if let Some(bytes) = self.bytes_uploaded {
            observations.push((BYTES_UPLOADED, as_bytes(bytes), BYTES_UNIT));
        }
        observations
    }
}

/// 1 for a request of the class an error counter counts, 0 for one that is not of it.
fn of_the_class(in_class: bool) -> f64 {
    if in_class {
        ONE_REQUEST
    } else {
        NOT_OF_THE_CLASS
    }
}

/// `bytes` as the value a byte counter carries.
#[allow(clippy::cast_precision_loss)]
fn as_bytes(bytes: usize) -> f64 {
    // A body longer than f64's exact integers cannot arise: this service buffers every body
    // it serves in memory, under the byte budget its own store enforces.
    bytes as f64
}

/// `span` as the value a latency carries: the measurement, in the milliseconds the two latency
/// series are published in.
fn as_millis(span: Duration) -> f64 {
    span.as_secs_f64() * MILLIS_PER_SECOND
}

/// One modelled operation, served with its answers COUNTED into the request metrics of the
/// bucket the request addressed.
///
/// The operation behind it is untouched: it owns deserialize -> validate -> handle ->
/// serialize exactly as before, and the counting happens once its answer exists — which is
/// why the status counted is the one the caller really received, whether a handler or the
/// framework's own validating walk produced it.
pub struct Metered<Eng: Send + Sync + 'static, O> {
    operation: O,
    /// The SAME shared engine the operation reads: the bucket's configurations are read from
    /// this service's store and the datapoints are appended to CloudWatch's, both through it.
    engine: Arc<Eng>,
    /// Which type of request this operation's requests are.
    kind: RequestKind,
}

impl<Eng: Send + Sync + 'static, O> Metered<Eng, O> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: O, engine: Arc<Eng>, kind: RequestKind) -> Self {
        Self {
            operation,
            engine,
            kind,
        }
    }
}

impl<Eng, O> ErasedOperation for Metered<Eng, O>
where
    Eng: Send + Sync + 'static,
    O: ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Serving this request begins here, which is where the span both latency series
            // measure begins: the request has been received in full by the time an operation is
            // reached.
            let clock = RequestClock::started();
            let response = self.operation.run(svc, req, ctx).await;
            // The answer's first byte exists the moment the operation hands its response back.
            let clock = clock.first_byte();
            // Nothing borrowed from either store is held across an await: the whole fold runs
            // after the answer exists and before it is handed back.
            count_served_request(&*self.engine, req, &response, self.kind, clock);
            response
        })
    }

    /// The wrapped operation's modelled auth schemes: counting an answer must not change
    /// which schemes sign the request.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Count one answer this service served into the request metrics of the bucket it was
/// addressed to.
///
/// Nothing is published when the request addressed no bucket, when the bucket carries no
/// metrics configuration the request falls under, or when the CloudWatch store is not mounted
/// beside this service: a datapoint has one home, and this path never makes a second one.
pub fn count_served_request<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    response: &RtResponse,
    kind: RequestKind,
    clock: RequestClock,
) {
    let path = path_style_path(engine, req);
    let Some(bucket) = super::addressed_bucket_name(&path) else {
        return;
    };
    let key = super::wire::addressed_object(&path, super::wire::request_query(req.uri()))
        .map(|object| object.key);
    let filter_ids = counting_configurations(engine, &bucket, key.as_deref());
    if filter_ids.is_empty() {
        return;
    }
    let served = ServedRequest {
        kind,
        status: response.status().as_u16(),
        bytes_downloaded: carried_bytes(response.body().bytes()),
        bytes_uploaded: carried_bytes(req.body().bytes()),
        // Read once the whole answer, its body included, is in hand: that is the last byte of
        // the request the whole-request latency is measured to.
        latency: clock.latency(),
    };
    publish(engine, &bucket, &filter_ids, &served, SystemTime::now());
}

/// How many bytes a body carries, for a body that carries any — `None` for an empty body and
/// for one this process cannot measure without consuming it, neither of which is a body a
/// byte counter counts.
fn carried_bytes(body: Option<&[u8]>) -> Option<usize> {
    body.map(<[u8]>::len).filter(|bytes| *bytes > 0)
}

/// The address `req` names, in the PATH-STYLE form (`/{Bucket}`, `/{Bucket}/{Key+}`): a
/// virtual-hosted-style request carries its bucket in the host and the key alone in the path,
/// so the bucket is put back in front of it — the same normalization the routing seam reads an
/// address with ([`super::EmulatedService::path_style_path`]), so the bucket a request is
/// SERVED for and the bucket its answer is counted into cannot differ.
fn path_style_path<'r, Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &'r RtRequest,
) -> Cow<'r, str> {
    let path = super::wire::request_path(req.uri());
    let addressed = super::request_host_of(req).and_then(|host| {
        super::addressed_in_host(host, path, &|name| super::holds_bucket_in(engine, name))
    });
    match addressed {
        Some(bucket) => Cow::Owned(super::path_with_bucket(bucket, path)),
        None => Cow::Borrowed(path),
    }
}

/// The ids of the metrics configurations of `bucket` that a request addressing `key` is
/// counted under, in the store's own id order.
///
/// Empty for a bucket this emulator does not hold and for one carrying no configuration the
/// request falls under, which is the bucket that publishes no request metrics at all.
fn counting_configurations<Eng: Send + Sync + 'static>(
    engine: &Eng,
    bucket: &str,
    key: Option<&str>,
) -> Vec<String> {
    let Some(state) = crate::state::service_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    let Some(held) = store.bucket(bucket) else {
        return Vec::new();
    };
    // A TAG predicate is about the object the request addressed, so it is judged against that
    // object's own tag set as this store holds it when the request is counted; a key the store
    // holds no object under carries no tag, which is what a request for an object that is not
    // there addresses.
    let tags: Vec<ResourceTag> = key
        .and_then(|key| held.object(key))
        .map(|object| object.tags.clone())
        .unwrap_or_default();
    held.metrics_configurations()
        .filter(|(_, configuration)| matches(configuration.filter.as_ref(), key, &tags))
        .map(|(id, _)| id.to_string())
        .collect()
}

/// Whether a request addressing `key`, whose object carries `tags`, falls under a metrics
/// configuration filtered by `filter`.
///
/// The rule is `MetricsFilter`'s own: a configuration that states no filter covers the whole
/// bucket, and a filter is "a prefix, an object tag, an access point ARN, or a conjunction
/// (MetricsAndOperator)" — the conjunction holding where every predicate it names holds.
///
/// An ACCESS POINT predicate selects the requests that reached the bucket THROUGH that access
/// point. This emulator addresses a bucket by its name, so no request it serves arrives
/// through an access point and such a predicate selects none of them — the same as a prefix
/// nothing is addressed under.
pub fn matches(filter: Option<&MetricsFilter>, key: Option<&str>, tags: &[ResourceTag]) -> bool {
    match filter {
        None => true,
        Some(MetricsFilter::Prefix(prefix)) => under_prefix(key, prefix),
        Some(MetricsFilter::Tag(tag)) => carries_tag(tags, tag),
        Some(MetricsFilter::AccessPointArn(_)) => false,
        Some(MetricsFilter::And(and)) => {
            and.prefix
                .as_ref()
                .is_none_or(|prefix| under_prefix(key, prefix))
                && and
                    .tags
                    .iter()
                    .flatten()
                    .all(|tag| carries_tag(tags, tag))
                && and.access_point_arn.is_none()
        }
    }
}

/// Whether the request addressed an object whose key begins with `prefix`. A request that
/// addresses no object at all (a listing, a bucket-level call) is under no prefix: the
/// predicate is about a key, and such a request names none.
fn under_prefix(key: Option<&str>, prefix: &str) -> bool {
    key.is_some_and(|key| key.starts_with(prefix))
}

/// Whether `tags` carries exactly `tag`: a tag predicate is the key AND the value, the way
/// `Tag` declares the pair.
fn carries_tag(tags: &[ResourceTag], tag: &Tag) -> bool {
    tags.iter()
        .any(|held| held.key == tag.key && held.value == tag.value)
}

/// Append `served`'s observations to the series of every configuration it was counted under.
fn publish<Eng: Send + Sync + 'static>(
    engine: &Eng,
    bucket: &str,
    filter_ids: &[String],
    served: &ServedRequest,
    at: SystemTime,
) {
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let timestamp_secs = epoch_secs(at);
    let observations = served.observations();
    let mut store = state.lock();
    for filter_id in filter_ids {
        let dimensions: cloudwatch::state::DimensionSet = [
            (BUCKET_NAME_DIMENSION.to_string(), bucket.to_string()),
            (FILTER_ID_DIMENSION.to_string(), filter_id.clone()),
        ]
        .into_iter()
        .collect();
        for (metric_name, value, unit) in &observations {
            let key = cloudwatch::state::MetricKey {
                namespace: NAMESPACE.to_string(),
                metric_name: (*metric_name).to_string(),
                dimensions: dimensions.clone(),
            };
            let datum = cloudwatch::state::StoredDatum {
                // CloudWatch holds a datapoint's instant as epoch SECONDS, which is the unit
                // its own readers window by.
                timestamp_secs,
                // One observation: this request's own contribution to the series.
                sample_count: 1.0,
                sum: *value,
                minimum: *value,
                maximum: *value,
                unit: Some((*unit).to_string()),
                storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
            };
            if store.append_datapoints(key, vec![datum]).is_err() {
                // The metric store holds no more series. The request itself was already
                // served, so nothing is failed for it; the dropped observation is reported
                // where a developer can see it.
                ::tracing::warn!(
                    bucket = %bucket,
                    filter = %filter_id,
                    metric = %metric_name,
                    "the CloudWatch metric store is at capacity, so this request was not counted"
                );
            }
        }
    }
}

/// `at` as the epoch seconds a stored datapoint carries.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// Serve every operation whose answers are counted into a bucket's request metrics, wrapped
/// in [`Metered`], under the modelled name the protocol resolves for it.
///
/// WHICH operations those are, and which type each one's requests are: the object data plane
/// — the reads, writes and deletes of a key, the multipart upload that writes one in parts.
/// The two copies, which this service already serves behind its front door for the refusal
/// only a door can word, are wrapped where they are built ([`super::front_door`]) so that one
/// operation is not served twice; the four listings of a bucket's contents, which need a door
/// for the same reason and are MANAGEMENT events besides, are wrapped where their record is
/// built ([`super::management_events::audited_table`]).
///
/// Not here, and why: `DeleteObjects` names the objects it deletes in its request DOCUMENT
/// rather than in its address, so which filtered configurations its keys fall under is not a
/// question about the request's address, and the browser-form `POST` of an object
/// ([`super::post_object`]) carries its key in the form document for the same reason; the
/// bucket's own configuration plane (its tagging, its policy, its own metrics configurations)
/// is not a request on the bucket's contents.
pub fn serve_metered<Eng: Send + Sync + 'static>(
    table: &mut OperationRoutingTable,
    engine: &Arc<Eng>,
    data_events: &Arc<super::data_events::ObjectDataEvents<Eng>>,
) {
    meter(
        table,
        engine,
        data_events,
        crate::ops::get_object::GetObject::new(Arc::clone(engine)),
        RequestKind::Get,
    );
    meter(
        table,
        engine,
        data_events,
        crate::ops::head_object::HeadObject::new(Arc::clone(engine)),
        RequestKind::Head,
    );
    meter(
        table,
        engine,
        data_events,
        crate::ops::put_object::PutObject::new(Arc::clone(engine)),
        RequestKind::Put,
    );
    meter(
        table,
        engine,
        data_events,
        crate::ops::delete_object::DeleteObject::new(Arc::clone(engine)),
        RequestKind::Delete,
    );
    meter(
        table,
        engine,
        data_events,
        crate::ops::upload_part::UploadPart::new(Arc::clone(engine)),
        RequestKind::Put,
    );
    meter(
        table,
        engine,
        data_events,
        crate::ops::create_multipart_upload::CreateMultipartUpload::new(Arc::clone(engine)),
        RequestKind::Post,
    );
    meter(
        table,
        engine,
        data_events,
        crate::ops::complete_multipart_upload::CompleteMultipartUpload::new(Arc::clone(engine)),
        RequestKind::Post,
    );
    meter(
        table,
        engine,
        data_events,
        crate::ops::abort_multipart_upload::AbortMultipartUpload::new(Arc::clone(engine)),
        RequestKind::Delete,
    );
    // The four listings of what a bucket holds are counted AND served behind a door AND
    // recorded as management events of this service, so they are served where that record is
    // built ([`super::management_events::audited_table`], which composes
    // [`counted_behind_door`] inside its recorder): one entry per operation, so the counting,
    // the door and the record are the same served operation rather than three.
}

/// Add one metered operation to `table` behind this service's FRONT DOOR, keyed by its own
/// modelled name.
///
/// What the door buys, and why these operations need it: a value the BOUNDARY rejects is
/// otherwise answered by the framework's own renderer, which cannot write the members S3's
/// document carries beside the code and the message — the `<ArgumentName>` /
/// `<ArgumentValue>` pair naming the refused parameter — nor the pair of request-correlation
/// ids every other answer of this service is named by. Behind the door that refusal is worded
/// by [`super::uri_value_wording`] and rendered by this service
/// (`super::EmulatedService::detailed_refusal`).
pub(super) fn meter_behind_door<Eng, O>(
    table: &mut OperationRoutingTable,
    engine: &Arc<Eng>,
    data_events: &Arc<super::data_events::ObjectDataEvents<Eng>>,
    operation: O,
    kind: RequestKind,
) where
    Eng: Send + Sync + 'static,
    O: Operation + ErasedOperation + 'static,
{
    // The same split [`meter`] makes, for the same reason: a call on one object's contents is
    // a DATA event of the account, recorded inside this wrapper and delivered outside it.
    let entry: Box<dyn ErasedOperation> = match super::data_events::recorded_class(O::NAME) {
        Some(class) => super::data_events::served_as_data_event(
            data_events,
            operation,
            class,
            |audited| counted_behind_door(audited, engine, kind),
        ),
        None => Box::new(counted_behind_door(operation, engine, kind)),
    };
    table.add_operation(O::NAME, served_to_every_caller::<O>(entry));
}

/// `operation` behind this service's front door, with its answers counted — the pair
/// [`meter_behind_door`] serves, as a FUNCTION so the two arms above can wrap values of two
/// different types (a closure is monomorphic in its argument).
///
/// Reached from the AUDIT table as well ([`super::management_events`]), which serves the
/// listings of a bucket's contents through this same pair with the record built inside it.
pub(super) fn counted_behind_door<Eng, O>(
    operation: O,
    engine: &Arc<Eng>,
    kind: RequestKind,
) -> Metered<Eng, FrontDoor<O>>
where
    Eng: Send + Sync + 'static,
    O: Operation + ErasedOperation,
{
    Metered::new(
        FrontDoor::worded(operation, super::submitted_value_wording)
            .wording_uri(super::uri_value_wording),
        Arc::clone(engine),
        kind,
    )
}

/// Add one metered operation to `table`, keyed by its own modelled name — the key the
/// generated table is keyed by, so a wrapped operation cannot be filed under a key the
/// protocol never resolves.
fn meter<Eng, O>(
    table: &mut OperationRoutingTable,
    engine: &Arc<Eng>,
    data_events: &Arc<super::data_events::ObjectDataEvents<Eng>>,
    operation: O,
    kind: RequestKind,
) where
    Eng: Send + Sync + 'static,
    O: Operation + ErasedOperation + 'static,
{
    // A call on the CONTENTS of one object is also a DATA event of the account, which the
    // catalog states once ([`super::data_events::DATA_EVENT_OPERATIONS`]): such an operation
    // is served with its record built inside this wrapper and delivered outside it, and every
    // other one is served counted and nothing more.
    let entry: Box<dyn ErasedOperation> = match super::data_events::recorded_class(O::NAME) {
        Some(class) => super::data_events::served_as_data_event(
            data_events,
            operation,
            class,
            |audited| Metered::new(audited, Arc::clone(engine), kind),
        ),
        None => Box::new(Metered::new(operation, Arc::clone(engine), kind)),
    };
    table.add_operation(O::NAME, served_to_every_caller::<O>(entry));
}

/// `entry` wrapped so that the callers THIS SERVICE admits reach it too — the ANONYMOUS caller
/// for the operations that judge it themselves
/// ([`super::unsigned_request::served_to_the_anonymous_caller`] is the catalog), and the caller
/// whose credential travels in a presentation the framework's authenticator cannot read, whose
/// signature this service recomputes itself
/// ([`super::legacy_query_credential`]).
///
/// Applied OUTSIDE the counting and the data-event record, because admitting the caller is what
/// decides whether the request is served at all: an anonymous read that IS served is counted
/// into the addressed bucket's request metrics and recorded as that object's data event exactly
/// as the owner's own read is.
fn served_to_every_caller<O: Operation>(entry: Box<dyn ErasedOperation>) -> Box<dyn ErasedOperation> {
    Box::new(super::unsigned_request::ServedUnsigned::new(
        O::NAME, entry,
    ))
}

#[cfg(test)]
mod tests {
    //! What one served answer contributes to each series, and which configurations one
    //! request falls under. The published series themselves are pinned end to end over the
    //! wire in `tests/all/request_metrics.rs`.

    use super::*;
    use crate::ops::types::MetricsAndOperator;

    /// The value `metric` takes in `served`'s observations, or `None` when this request
    /// observes that series at all.
    fn value_of(served: &ServedRequest, metric: &str) -> Option<f64> {
        served
            .observations()
            .into_iter()
            .find(|(name, _, _)| *name == metric)
            .map(|(_, value, _)| value)
    }

    /// One tag, as a filter and as a stored tag set spell it.
    fn tag(key: &str, value: &str) -> Tag {
        Tag {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    fn held_tag(key: &str, value: &str) -> ResourceTag {
        ResourceTag {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    /// A measured pair of spans, as a clock hands them over.
    fn took(to_first_byte_micros: u64, total_micros: u64) -> ServedLatency {
        ServedLatency {
            to_first_byte: Duration::from_micros(to_first_byte_micros),
            total: Duration::from_micros(total_micros),
        }
    }

    #[test]
    fn an_error_counter_is_one_for_its_own_class_and_zero_for_every_other() {
        // Rows span the status classes the error counters split requests into, including the
        // boundaries of each class.
        let cases: [(u16, f64, f64); 8] = [
            (200, 0.0, 0.0),
            (204, 0.0, 0.0),
            (304, 0.0, 0.0),
            (400, 1.0, 0.0),
            (404, 1.0, 0.0),
            (499, 1.0, 0.0),
            (500, 0.0, 1.0),
            (503, 0.0, 1.0),
        ];
        for (status, four_xx, five_xx) in cases {
            let served = ServedRequest {
                kind: RequestKind::Get,
                status,
                bytes_downloaded: None,
                bytes_uploaded: None,
                latency: took(400, 650),
            };
            assert_eq!(
                value_of(&served, FOUR_XX_ERRORS),
                Some(four_xx),
                "case {status}: expected a 4xxErrors observation of {four_xx}"
            );
            assert_eq!(
                value_of(&served, FIVE_XX_ERRORS),
                Some(five_xx),
                "case {status}: expected a 5xxErrors observation of {five_xx}"
            );
            assert_eq!(
                value_of(&served, ALL_REQUESTS),
                Some(1.0),
                "case {status}: every answer is one request of the bucket"
            );
        }
    }

    #[test]
    fn a_request_is_counted_into_the_counter_of_its_own_type_alone() {
        // Rows span every type a counted request can be.
        let cases: [(RequestKind, &str); 6] = [
            (RequestKind::Get, GET_REQUESTS),
            (RequestKind::Put, PUT_REQUESTS),
            (RequestKind::Delete, DELETE_REQUESTS),
            (RequestKind::Head, HEAD_REQUESTS),
            (RequestKind::Post, POST_REQUESTS),
            (RequestKind::List, LIST_REQUESTS),
        ];
        let every_counter = [
            GET_REQUESTS,
            PUT_REQUESTS,
            DELETE_REQUESTS,
            HEAD_REQUESTS,
            POST_REQUESTS,
            LIST_REQUESTS,
        ];
        for (kind, counter) in cases {
            let served = ServedRequest {
                kind,
                status: 200,
                bytes_downloaded: None,
                bytes_uploaded: None,
                latency: took(400, 650),
            };
            for candidate in every_counter {
                let expected = (candidate == counter).then_some(1.0);
                let actual = value_of(&served, candidate);
                assert_eq!(
                    actual, expected,
                    "case {kind:?}: expected {candidate} to be {expected:?}, got {actual:?}"
                );
            }
        }
    }

    #[test]
    fn a_byte_counter_counts_only_a_request_that_carried_a_body_that_way() {
        // Rows span the four combinations of a body in each direction, which is what each
        // byte counter's definition selects on.
        let cases: [(&str, Option<usize>, Option<usize>); 4] = [
            ("a read that served bytes", Some(11), None),
            ("a write that sent bytes", None, Some(7)),
            ("a head that carried neither", None, None),
            ("an answer in both directions", Some(3), Some(5)),
        ];
        for (case, downloaded, uploaded) in cases {
            let served = ServedRequest {
                kind: RequestKind::Get,
                status: 200,
                bytes_downloaded: downloaded,
                bytes_uploaded: uploaded,
                latency: took(400, 650),
            };
            let expected_down = downloaded.map(as_bytes);
            let actual_down = value_of(&served, BYTES_DOWNLOADED);
            assert_eq!(
                actual_down, expected_down,
                "case {case:?}: expected BytesDownloaded {expected_down:?}, got {actual_down:?}"
            );
            let expected_up = uploaded.map(as_bytes);
            let actual_up = value_of(&served, BYTES_UPLOADED);
            assert_eq!(
                actual_up, expected_up,
                "case {case:?}: expected BytesUploaded {expected_up:?}, got {actual_up:?}"
            );
        }
    }

    #[test]
    fn every_request_observes_both_latencies_as_the_milliseconds_they_were_measured_in() {
        // Rows span the spans a measurement can be: sub-millisecond, whole milliseconds, a
        // fraction of one, and a slow request — each with a whole-request span of its own.
        let cases: [(&str, u64, u64); 5] = [
            ("a sub-millisecond request", 120, 180),
            ("a whole millisecond", 1_000, 2_000),
            ("a fraction over a millisecond", 1_500, 1_750),
            ("a slow request", 250_000, 400_000),
            ("one span the whole request is", 900, 900),
        ];
        for (case, to_first_byte, total) in cases {
            let served = ServedRequest {
                kind: RequestKind::Get,
                status: 200,
                bytes_downloaded: Some(4),
                bytes_uploaded: None,
                latency: took(to_first_byte, total),
            };
            // Micros in a millisecond: the rows are written in the finer unit, the series are
            // published in the coarser one.
            const MICROS_PER_MILLI: f64 = 1_000.0;
            // The slack a millisecond read out of a span of micros is compared within: a
            // conversion between two binary fractions, not a wall-clock allowance.
            const SLACK_MS: f64 = 1e-9;
            #[allow(clippy::cast_precision_loss)]
            let expected = [
                (FIRST_BYTE_LATENCY, to_first_byte as f64 / MICROS_PER_MILLI),
                (TOTAL_REQUEST_LATENCY, total as f64 / MICROS_PER_MILLI),
            ];
            for (metric, expected) in expected {
                let actual = value_of(&served, metric);
                assert!(
                    actual.is_some_and(|actual| (actual - expected).abs() < SLACK_MS),
                    "case {case:?}: expected {metric} to be {expected} ms, got {actual:?}"
                );
            }
            for metric in [FIRST_BYTE_LATENCY, TOTAL_REQUEST_LATENCY] {
                let unit = served
                    .observations()
                    .into_iter()
                    .find(|(name, _, _)| *name == metric)
                    .map(|(_, _, unit)| unit);
                assert_eq!(
                    unit,
                    Some(MILLISECONDS_UNIT),
                    "case {case:?}: expected {metric} in {MILLISECONDS_UNIT}, got {unit:?}"
                );
            }
        }
    }

    #[test]
    fn a_clock_measures_the_first_byte_span_inside_the_whole_request_span() {
        // The clock is the only source of a latency, so what it hands over is what a published
        // observation can be: a first-byte span no longer than the whole-request span, and a
        // published value of each that a reader can window and aggregate.
        let clock = RequestClock::started().first_byte();
        let latency = clock.latency();
        assert!(
            latency.to_first_byte <= latency.total,
            "the first-byte span must lie inside the whole-request span, got \
             {latency:?}"
        );
        for (metric, span) in [
            (FIRST_BYTE_LATENCY, latency.to_first_byte),
            (TOTAL_REQUEST_LATENCY, latency.total),
        ] {
            let published = as_millis(span);
            assert!(
                published.is_finite() && published >= 0.0,
                "case {metric}: a measured span publishes a finite, non-negative number of \
                 milliseconds, got {published} from {span:?}"
            );
        }
    }

    #[test]
    fn a_filter_selects_the_requests_its_own_predicates_hold_for() {
        let tags = [held_tag("tier", "gold"), held_tag("team", "ops")];
        // Rows span every form `MetricsFilter` declares, each against a key it selects and a
        // key it does not.
        let cases: [(&str, Option<MetricsFilter>, Option<&str>, bool); 12] = [
            ("no filter, a key", None, Some("loud/one"), true),
            ("no filter, no key", None, None, true),
            (
                "a prefix the key is under",
                Some(MetricsFilter::Prefix("loud/".to_string())),
                Some("loud/one"),
                true,
            ),
            (
                "a prefix the key is not under",
                Some(MetricsFilter::Prefix("loud/".to_string())),
                Some("quiet/one"),
                false,
            ),
            (
                "the empty prefix, which every key is under",
                Some(MetricsFilter::Prefix(String::new())),
                Some("quiet/one"),
                true,
            ),
            (
                "a prefix and no key at all",
                Some(MetricsFilter::Prefix("loud/".to_string())),
                None,
                false,
            ),
            (
                "a tag the object carries",
                Some(MetricsFilter::Tag(tag("tier", "gold"))),
                Some("loud/one"),
                true,
            ),
            (
                "the same key with another value",
                Some(MetricsFilter::Tag(tag("tier", "silver"))),
                Some("loud/one"),
                false,
            ),
            (
                "an access point ARN, which no addressed request reaches",
                Some(MetricsFilter::AccessPointArn(
                    "arn:aws:s3:us-east-1:000000000000:accesspoint/reports".to_string(),
                )),
                Some("loud/one"),
                false,
            ),
            (
                "a conjunction every predicate of which holds",
                Some(MetricsFilter::And(MetricsAndOperator {
                    prefix: Some("loud/".to_string()),
                    tags: Some(vec![tag("tier", "gold"), tag("team", "ops")]),
                    access_point_arn: None,
                })),
                Some("loud/one"),
                true,
            ),
            (
                "a conjunction one predicate of which does not",
                Some(MetricsFilter::And(MetricsAndOperator {
                    prefix: Some("loud/".to_string()),
                    tags: Some(vec![tag("tier", "gold"), tag("team", "finance")]),
                    access_point_arn: None,
                })),
                Some("loud/one"),
                false,
            ),
            (
                "a conjunction naming an access point",
                Some(MetricsFilter::And(MetricsAndOperator {
                    prefix: Some("loud/".to_string()),
                    tags: None,
                    access_point_arn: Some(
                        "arn:aws:s3:us-east-1:000000000000:accesspoint/reports".to_string(),
                    ),
                })),
                Some("loud/one"),
                false,
            ),
        ];
        for (case, filter, key, expected) in cases {
            let actual = matches(filter.as_ref(), key, &tags);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the filter to select the request == {expected}, \
                 got {actual}"
            );
        }
    }
}
