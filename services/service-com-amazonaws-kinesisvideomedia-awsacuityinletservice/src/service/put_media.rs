// SPDX-License-Identifier: CC0-1.0

//! The PUT_MEDIA ingest plane: `POST /putMedia`, the producer half of a Kinesis video
//! stream.
//!
//! # Not a modelled operation
//!
//! `PutMedia` is in no Smithy model this emulator carries: the Kinesis Video Streams
//! control plane binds 32 operations and none of them is it, and this service's own
//! model binds `GetMedia` alone (`smithy_query(query="digest")` on either model). The
//! ingest front end is recorded behaviour, and the control plane names it: an `APIName`
//! of `PUT_MEDIA` resolves this service's own data endpoint
//! (`service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/src/state/data_endpoint.rs`),
//! and `GetMedia`'s own documentation points at it — "When you put media data
//! (fragments) on a stream, Kinesis Video Streams stores each incoming fragment and
//! related metadata in what is called a chunk. For more information, see PutMedia".
//!
//! Nothing in the generated routing table can name a request with no `X-Amz-Target` and
//! no modelled URI, so the ingest plane is reached through the two seams
//! [`crate::service::EmulatedService`] has for exactly that: it is CLAIMED by
//! [`claims_parts`] (the route key is this family's signing name, the first path segment
//! is `putMedia`) and SERVED as the one [`ErasedOperation`] below, which declares SigV4
//! so the emulator verifies the producer's signature before any media is taken.
//!
//! # The wire
//!
//! A producer addresses the stream in request HEADERS rather than in a body, because the
//! body is the media: `x-amzn-stream-name` or `x-amzn-stream-arn` names the stream,
//! `x-amzn-fragment-timecode-type` says whether the container's Cluster timecodes are
//! RELATIVE to `x-amzn-producer-start-timestamp` or ABSOLUTE epoch milliseconds, and
//! the body is a Matroska container in which one Cluster is one fragment
//! ([`crate::service::matroska`]).
//!
//! The answer is `200` whose body is the stream of JSON acknowledgements the producer
//! reads its fragments' fate from: for each accepted Cluster a `BUFFERING`, a `RECEIVED`
//! and a `PERSISTED` acknowledgement, carrying the fragment number this service minted
//! and that Cluster's own timecode. They are in the order the events happen in — a
//! fragment is received as the container streams in and stored behind that, so every
//! Cluster's receipt is acknowledged before any Cluster's storing. A fault in the
//! CONTAINER is reported in band, as an `ERROR` acknowledgement inside that same `200`,
//! and the session stops at the Cluster that carried it, so the Clusters accepted before
//! it stay ingested. A fault in the REQUEST — a stream nothing registered, a missing
//! addressing header — is an HTTP error, in this service's own modelled error
//! vocabulary.
//!
//! Every accepted Cluster is persisted through
//! [`crate::state::ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::ingest_fragment`],
//! the one write of the producer plane, so a fragment this plane acknowledges is the
//! fragment `GetMedia`, `ListFragments` and `GetMediaForFragmentList` go on to report.

use std::sync::Arc;
use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::service::unknown_operation as control_plane_front_door;
use ::service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::state as control_plane;
use ::service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader::state as archive;
use ::smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

use crate::service::matroska::{self, ContainerFault, SubmittedCluster};
use crate::state;

/// The first URI path segment the ingest plane is addressed at: a producer appends the
/// API name to the data endpoint `GetDataEndpoint(APIName=PUT_MEDIA)` handed it.
pub const PUT_MEDIA_PATH_SEGMENT: &str = "putMedia";

/// The header naming the stream to ingest into, by name.
const STREAM_NAME_HEADER: &str = "x-amzn-stream-name";

/// The header naming the stream to ingest into, by ARN.
const STREAM_ARN_HEADER: &str = "x-amzn-stream-arn";

/// The header saying how the container's Cluster timecodes are to be read.
const TIMECODE_TYPE_HEADER: &str = "x-amzn-fragment-timecode-type";

/// The header carrying the producer's clock at the start of the session, in epoch
/// seconds — what a RELATIVE Cluster timecode is relative to.
const PRODUCER_START_TIMESTAMP_HEADER: &str = "x-amzn-producer-start-timestamp";

/// Cluster timecodes are an offset from the producer start timestamp.
const TIMECODE_TYPE_RELATIVE: &str = "RELATIVE";

/// Cluster timecodes are epoch milliseconds.
const TIMECODE_TYPE_ABSOLUTE: &str = "ABSOLUTE";

/// The media type the acknowledgement stream is served as: the body is a stream of JSON
/// objects.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The HTTP field this emulator states an error code in, which is also where a Kinesis
/// Video Streams media API states one — "a more specific error type in addition to what
/// the HTTP status code provides" (`GetMedia`'s documentation). The name is the one this
/// emulator's own error rendering writes, and field names are case-insensitive, so a
/// client matches it however the transport cases the bytes.
const ERROR_TYPE_FIELD: &str = "x-amzn-errortype";

/// `InvalidArgumentException` — "The value for this input parameter is invalid" (HTTP
/// 400), the refusal of a request whose addressing headers name no one stream or whose
/// timecode type is outside the published value set.
const INVALID_ARGUMENT: &str = "InvalidArgumentException";

/// `ResourceNotFoundException` — "The stream with the given name does not exist" (HTTP
/// 404), the refusal of an ingest into a stream the control-plane registry does not
/// hold.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// `AccessDeniedException` — the code AWS refuses a call IAM does not permit with (HTTP
/// 403), answered ahead of everything this plane would otherwise read or write
/// ([`crate::service::authorization`]). No Kinesis Video media model declares a shape for
/// it, so it is stated here in this plane's own error envelope.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The code this emulator states for a fault of its OWN — the one it answers when a
/// handler cannot run at all (HTTP 500). It is not a limit the caller reached and not a
/// fault in the request, so it carries neither a throttling code nor a 4xx.
const INTERNAL_FAILURE: &str = "InternalFailure";

/// The API name this route is published and authorized under: `GetDataEndpoint`'s
/// `APIName` for the write plane, which is also the name of the IAM action it performs
/// (`kinesisvideo:PutMedia`).
const PUT_MEDIA_API_NAME: &str = "PutMedia";

/// The auth schemes a request to this plane is verified under.
///
/// A producer SigV4-signs `POST /putMedia`, and SigV4 is the only scheme the Kinesis
/// Video models declare. This is not a modelled operation, so there is no modelled
/// operation for the router to read schemes off: without this the router would find no
/// scheme for the request and could not verify the credential it carries.
const INGEST_AUTH_SCHEMES: &[&str] = &[::emulator_server::aws_credentials::SIGV4_SCHEME];

/// How a producer's Cluster timecodes are to be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimecodeType {
    /// An offset, in the container's timecode scale, from the producer start timestamp.
    Relative,
    /// An absolute instant, in the container's timecode scale, from the Unix epoch.
    Absolute,
}

impl TimecodeType {
    /// Read the wire value, or `None` when it is outside the published set.
    fn parse(value: &str) -> Option<Self> {
        match value {
            TIMECODE_TYPE_RELATIVE => Some(Self::Relative),
            TIMECODE_TYPE_ABSOLUTE => Some(Self::Absolute),
            _ => None,
        }
    }
}

/// What one ingest request says about itself, read from its headers.
#[derive(Debug, Clone)]
struct SubmittedSession {
    /// The `StreamName` the request named, when it named one.
    stream_name: Option<String>,
    /// The `StreamARN` the request named, when it named one.
    stream_arn: Option<String>,
    /// How the container's Cluster timecodes are to be read.
    timecode_type: TimecodeType,
    /// The producer's clock at the start of the session, in epoch milliseconds, when the
    /// request stated one. A RELATIVE Cluster timecode is an offset from it, so a RELATIVE
    /// session carries it; an ABSOLUTE one names the instant itself and needs none.
    producer_start_millis: Option<i64>,
}

impl SubmittedSession {
    /// When the producer says the fragment `cluster` carries was captured.
    ///
    /// A RELATIVE timecode is an offset from the producer's own start timestamp; an
    /// ABSOLUTE one already names the instant. Both are read at the container's timecode
    /// scale, which [`SubmittedCluster::timecode_millis`] has already applied.
    ///
    /// The instant is built by the archive read plane's `from_epoch_millis` — the one
    /// total reading of an epoch-millisecond offset this family states — rather than by a
    /// second conversion here, which would be the one that panics at the ends of the
    /// range.
    fn producer_instant(&self, cluster: &SubmittedCluster) -> SystemTime {
        let millis = match self.timecode_type {
            TimecodeType::Relative => self
                .producer_start_millis
                .unwrap_or_default()
                .saturating_add(cluster.timecode_millis),
            TimecodeType::Absolute => cluster.timecode_millis,
        };
        archive::from_epoch_millis(millis)
    }
}

/// One request-level refusal: the status, the error code a producer branches on, and the
/// sentence that states it.
#[derive(Debug, Clone)]
struct Refusal {
    status: u16,
    code: &'static str,
    message: String,
}

impl Refusal {
    /// This refusal on the wire, in the envelope a Kinesis Video Streams media API
    /// states an error in: the status, the error type field, and a `Message` member.
    fn response(&self) -> RtResponse {
        let body = ::serde_json::json!({ "Message": self.message }).to_string();
        let mut response = answer(self.status, body);
        response.headers_mut().insert(ERROR_TYPE_FIELD, self.code);
        response
    }
}

/// The ingest plane of this service, as the one [`ErasedOperation`] it is served
/// through.
///
/// Owns the shared engine exactly as a generated operation does, so a fragment it takes
/// lands in the SAME store `GetMedia` reads and the archive read plane reports.
pub struct PutMediaIngest<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> PutMediaIngest<Eng> {
    /// Build the ingest plane from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> PutMediaIngest<Eng> {
    /// Whether this plane is the one the request's own ADDRESS serves, by the rule
    /// [`ingest_served_at`] states. Read through the shared engine this plane holds,
    /// which is where the control plane records the address it announced for each plane.
    pub fn serves_the_addressed_plane(&self, req: &RtRequest) -> bool {
        ingest_served_at(&*self.engine, req)
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for PutMediaIngest<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.serve(req, ctx).await })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(INGEST_AUTH_SCHEMES)
    }
}

impl<Eng: Send + Sync + 'static> PutMediaIngest<Eng> {
    /// Serve one ingest request: take the Clusters its container carries, and answer the
    /// acknowledgement stream for them.
    pub async fn serve(&self, req: &RtRequest, ctx: &OperationContext<'_>) -> RtResponse {
        let session = match submitted_session(req) {
            Ok(session) => session,
            Err(refusal) => return refusal.response(),
        };
        // ONE instant dates everything this session persists: the server timestamp of
        // every fragment it takes.
        let received_at = SystemTime::now();
        // The router buffers every request before dispatch, so a body it cannot hand
        // back in memory is media this plane cannot read — the same thing an unreadable
        // container is, and reported the same way.
        let body = req.body().bytes().unwrap_or_default().to_vec();
        let Some(store) = state::service_state(&*self.engine) else {
            // Unreachable in this emulator: the router builds one engine and hands it to
            // every service. Reported as a failure of this emulator rather than as a
            // success, so a producer never reads an empty acknowledgement stream as "the
            // service took nothing because you sent nothing".
            return store_unavailable().response();
        };

        // ONE instant dates everything this session persists: the server timestamp of
        // every fragment it takes.
        let name = session.stream_name.as_deref();
        let arn = session.stream_arn.as_deref();
        let submitted_bytes = body.len();

        // IAM decides before any media is taken: AWS evaluates authorization at the IAM
        // layer ahead of the operation, so a producer whose identity-based policies do not
        // allow `kinesisvideo:PutMedia` on the stream it addresses is refused, and nothing
        // is read from the container, persisted or acknowledged. Asked while no store of
        // this service is locked.
        let resource = crate::service::authorization::AddressedStream::of(&*self.engine, name, arn);
        if let Some(refusal) = crate::service::authorization::authorization_refusal(
            &*self.engine,
            ctx,
            PUT_MEDIA_API_NAME,
            &resource,
        ) {
            return access_denied(refusal).response();
        }

        // The stream is resolved BEFORE the container is read: a stream nothing
        // registered is a fault in the request, which is answered as an HTTP error, and
        // nothing about the submitted media changes that answer. The guard is taken for
        // that resolution alone and released again, so the container is read while no
        // other caller of this service is held up.
        if store
            .lock()
            .resolve_stream(&*self.engine, name, arn)
            .is_none()
        {
            return stream_not_found(&session).response();
        }

        // Reading the container — up to one request's ceiling of media, re-framed Cluster
        // by Cluster — is CPU-bound, so it runs on the blocking pool and not on the async
        // worker: a worker busy inside it serves no other request and lands no timeout,
        // cancellation or shutdown signal.
        let read = ::tokio::task::spawn_blocking(move || matroska::read_clusters(&body)).await;
        let clusters = match read {
            Ok(Ok(clusters)) => clusters,
            // A container this plane cannot read is a fault in the media, which Kinesis
            // Video Streams reports IN BAND: the request itself was well formed, so the
            // answer is the acknowledgement stream carrying the fault.
            Ok(Err(fault)) => return acknowledgement_stream(&[container_fault(fault)]),
            // The read did not finish — the runtime is shutting down, or the task was
            // cancelled. Nothing was taken, and nothing is acknowledged.
            Err(_) => return store_unavailable().response(),
        };

        let mut store = store.lock();

        // The acknowledgement stream is in the order the session's events happen in: a
        // fragment is BUFFERING while the service is taking it and RECEIVED once it has
        // the whole of it, which is as the container streams in — and PERSISTED only
        // after it has been stored, which is behind the receiving. So the receipt of
        // every Cluster is acknowledged first, in submission order, and the storing of
        // them follows.
        let mut acknowledgements: Vec<::serde_json::Value> = Vec::new();
        let mut persisted: Vec<::serde_json::Value> = Vec::new();
        let mut fault: Option<::serde_json::Value> = None;
        let mut previous: Option<u64> = None;
        let mut ingested_frames = 0usize;
        for cluster in clusters {
            // The number this fragment was RECEIVED under, which every acknowledgement of
            // it carries — the fault acknowledgements below included, since the fault was
            // found in a fragment the service had already taken in.
            let received_as = store.received_fragment_number(&*self.engine, name, arn);
            // "A cluster timecode moving backwards" ends the session at that Cluster:
            // the fragment log is ordered by time, so a fragment that goes back cannot
            // be appended to it. The Clusters already taken stay ingested.
            if previous.is_some_and(|held| cluster.timecode < held) {
                fault = Some(error_acknowledgement(
                    FRAGMENT_TIMECODE_LESSER_THAN_PREVIOUS,
                    FRAGMENT_TIMECODE_LESSER_THAN_PREVIOUS_ID,
                    Some(cluster.timecode),
                    received_as.as_deref(),
                ));
                break;
            }
            // A frame on a track the container never declared cannot be played back: a
            // reader handed the chunk has no `TrackEntry` to decode it with. The session
            // ends at that Cluster.
            if let Some(track) = cluster.undeclared_track {
                ::tracing::debug!(
                    operation = "PutMedia",
                    track,
                    "a submitted block names a track the container did not declare"
                );
                fault = Some(error_acknowledgement(
                    TRACK_NUMBER_MISMATCH,
                    TRACK_NUMBER_MISMATCH_ID,
                    Some(cluster.timecode),
                    received_as.as_deref(),
                ));
                break;
            }
            previous = Some(cluster.timecode);
            let producer_instant = session.producer_instant(&cluster);
            // A fragment cannot have been captured at an instant the service has not
            // reached: the timecode names a moment that has not happened, so there is no
            // instant to record it at and the session ends at that Cluster. The service's
            // own clock is the only reference it has for "now", and a producer's runs a
            // little ahead or behind it ([`PRODUCER_CLOCK_SKEW_ALLOWANCE`]).
            if producer_instant > received_at + PRODUCER_CLOCK_SKEW_ALLOWANCE {
                fault = Some(error_acknowledgement(
                    INVALID_PRODUCER_TIMESTAMP,
                    INVALID_PRODUCER_TIMESTAMP_ID,
                    Some(cluster.timecode),
                    received_as.as_deref(),
                ));
                break;
            }
            let Some(fragment_number) = store.ingest_fragment(
                &*self.engine,
                name,
                arn,
                state::ProducedFragment {
                    chunk: cluster.document,
                    producer_timestamp: producer_instant,
                    server_timestamp: received_at,
                    duration_millis: cluster.span_millis,
                },
            ) else {
                // The stream resolved a moment ago and no longer does: the control plane
                // removed it mid-session, which is the same request-level fault.
                return stream_not_found(&session).response();
            };
            for event in [EVENT_BUFFERING, EVENT_RECEIVED] {
                acknowledgements.push(fragment_acknowledgement(
                    event,
                    cluster.timecode,
                    &fragment_number,
                ));
            }
            persisted.push(fragment_acknowledgement(
                EVENT_PERSISTED,
                cluster.timecode,
                &fragment_number,
            ));
            ingested_frames += cluster.frames;
        }
        drop(store);
        // The PERSISTED acknowledgements of the session, which a producer reads only
        // while the session is live: a fault ENDS it at the Cluster that carried the
        // fault, so a session that ended that way reports none at all. What it had
        // already taken stays ingested — the fragments are stored, it is the session the
        // fault closed.
        if fault.is_none() {
            acknowledgements.extend(persisted);
        }
        // The fault that ended the session is the last thing the producer reads: every
        // fragment taken before it is acknowledged as received, and nothing after it was
        // taken.
        acknowledgements.extend(fault);

        tracing::debug!(
            operation = "PutMedia",
            stream = ?name,
            stream_arn = ?arn,
            timecode_type = ?session.timecode_type,
            acknowledgements = acknowledgements.len(),
            frames = ingested_frames,
            submitted_bytes,
            "ingested a media session"
        );

        acknowledgement_stream(&acknowledgements)
    }
}

/// `EventType` of an acknowledgement reporting that the service is taking the fragment.
const EVENT_BUFFERING: &str = "BUFFERING";

/// `EventType` of an acknowledgement reporting that the service has the whole fragment.
const EVENT_RECEIVED: &str = "RECEIVED";

/// `EventType` of an acknowledgement reporting that the fragment is stored — the event
/// after which the read planes report it.
const EVENT_PERSISTED: &str = "PERSISTED";

/// `EventType` of an acknowledgement reporting a fault in the submitted media.
const EVENT_ERROR: &str = "ERROR";

/// `ErrorCode` of a Cluster whose timecode goes back before the Cluster before it.
const FRAGMENT_TIMECODE_LESSER_THAN_PREVIOUS: &str = "FRAGMENT_TIMECODE_LESSER_THAN_PREVIOUS";

/// `ErrorId` of [`FRAGMENT_TIMECODE_LESSER_THAN_PREVIOUS`].
const FRAGMENT_TIMECODE_LESSER_THAN_PREVIOUS_ID: u64 = 4004;

/// `ErrorCode` of a container that cannot be read as Matroska.
const INVALID_MKV_DATA: &str = "INVALID_MKV_DATA";

/// `ErrorId` of [`INVALID_MKV_DATA`].
const INVALID_MKV_DATA_ID: u64 = 4006;

/// `ErrorCode` of a fragment whose producer timestamp names an instant that has not
/// happened yet.
const INVALID_PRODUCER_TIMESTAMP: &str = "INVALID_PRODUCER_TIMESTAMP";

/// `ErrorId` of [`INVALID_PRODUCER_TIMESTAMP`].
const INVALID_PRODUCER_TIMESTAMP_ID: u64 = 4007;

/// `ErrorCode` of a frame written on a track number the container's `Tracks` never
/// declared.
const TRACK_NUMBER_MISMATCH: &str = "TRACK_NUMBER_MISMATCH";

/// `ErrorId` of [`TRACK_NUMBER_MISMATCH`].
const TRACK_NUMBER_MISMATCH_ID: u64 = 4010;

/// How far ahead of this service's own clock a fragment's producer timestamp may be and
/// still be taken.
///
/// A producer stamps its fragments from ITS clock, which runs a little ahead or behind the
/// service's, and the fragments of a live session are stamped as they are captured — so a
/// producer timestamp slightly ahead of the instant the service reads it is ordinary and
/// is ingested as sent. What is refused is a timestamp no clock difference accounts for.
///
/// AWS publishes no figure for that allowance, so this span is CHOSEN: wide enough that no
/// difference between two machines' clocks, or the span of one session, can earn a
/// refusal.
const PRODUCER_CLOCK_SKEW_ALLOWANCE: ::std::time::Duration =
    ::std::time::Duration::from_secs(60 * 60);

/// One acknowledgement of an accepted fragment: the event it reports, the Cluster
/// timecode the producer sent, and the fragment number this service minted.
///
/// One JSON object per acknowledgement, whose members are the acknowledgement's own:
/// that is the form a producer reads off this wire, and the form the recorded ingest
/// front end answers.
///
/// `FragmentTimecode` is the Cluster's timecode AS SENT — the recorded behaviour reports
/// the timecode the submitted Cluster carried (under ABSOLUTE, the epoch-millisecond
/// value that was sent), so it is echoed at the container's own scale and not rescaled.
/// The scaled value is what the fragment's producer timestamp is computed from
/// ([`SubmittedCluster::timecode_millis`]), which is where the read planes report it.
fn fragment_acknowledgement(
    event_type: &str,
    fragment_timecode: u64,
    fragment_number: &str,
) -> ::serde_json::Value {
    ::serde_json::json!({
        "EventType": event_type,
        "FragmentTimecode": fragment_timecode,
        "FragmentNumber": fragment_number,
    })
}

/// One acknowledgement of a fault in the submitted media: the code and id a producer
/// branches on, the timecode of the Cluster that carried it when the fault names one, and
/// the number the fragment it was found in was received under.
fn error_acknowledgement(
    error_code: &str,
    error_id: u64,
    fragment_timecode: Option<u64>,
    fragment_number: Option<&str>,
) -> ::serde_json::Value {
    let mut acknowledgement = ::serde_json::Map::new();
    acknowledgement.insert("EventType".to_string(), EVENT_ERROR.into());
    // The timecode is stated only when the fault names a Cluster: a container that does
    // not parse has none, and a zero would name the session's first instant.
    if let Some(timecode) = fragment_timecode {
        acknowledgement.insert("FragmentTimecode".to_string(), timecode.into());
    }
    // The fragment number likewise: a fault in a fragment the service took in names that
    // fragment, while a container it could not read holds no fragment to name.
    if let Some(number) = fragment_number {
        acknowledgement.insert("FragmentNumber".to_string(), number.into());
    }
    acknowledgement.insert("ErrorId".to_string(), error_id.into());
    acknowledgement.insert("ErrorCode".to_string(), error_code.into());
    ::serde_json::Value::Object(acknowledgement)
}

/// The acknowledgement a container this plane could not read earns.
fn container_fault(fault: ContainerFault) -> ::serde_json::Value {
    match fault {
        ContainerFault::Unreadable => {
            // No fragment was taken in: a container whose tree does not read holds no
            // Cluster this plane could receive, so the fault names neither a timecode nor
            // a fragment number.
            error_acknowledgement(INVALID_MKV_DATA, INVALID_MKV_DATA_ID, None, None)
        }
    }
}

/// The acknowledgement stream, as the response body: one JSON object per
/// acknowledgement, in the order the session produced them.
fn acknowledgement_stream(acknowledgements: &[::serde_json::Value]) -> RtResponse {
    let mut body = String::new();
    for acknowledgement in acknowledgements {
        body.push_str(&acknowledgement.to_string());
        body.push('\n');
    }
    answer(200, body)
}

/// One response: the status, the JSON media type, and the body.
fn answer(status: u16, body: String) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    response
        .headers_mut()
        .insert("content-type", JSON_CONTENT_TYPE);
    response
}

/// The refusal of a request this plane could not even begin to serve: the store this
/// emulator was assembled without, or a container read the runtime did not finish.
///
/// Neither is a limit the producer reached and neither is a fault in its request, so this
/// is the emulator's own internal failure rather than a throttle: a retry of a throttle
/// would be a retry of something that will never clear.
fn store_unavailable() -> Refusal {
    Refusal {
        status: 500,
        code: INTERNAL_FAILURE,
        message: "The media store is unavailable.".to_string(),
    }
}

/// `AccessDeniedException` for a producer IAM does not permit: the sentence the gate
/// stated, in this plane's own error envelope at the status AWS refuses such a call with.
fn access_denied(message: String) -> Refusal {
    Refusal {
        status: 403,
        code: ACCESS_DENIED,
        message,
    }
}

/// `ResourceNotFoundException` for the stream this session named.
fn stream_not_found(_session: &SubmittedSession) -> Refusal {
    Refusal {
        status: 404,
        code: RESOURCE_NOT_FOUND,
        // The one sentence this family states for a stream that is not there to serve,
        // held by the archive read plane and stated by the control plane too — fixed text
        // carrying nothing from the request, as the recorded refusal carries nothing.
        message: archive::STREAM_NOT_FOUND_MESSAGE.to_string(),
    }
}

/// What the request's headers say about this session, or the refusal they earn.
fn submitted_session(req: &RtRequest) -> Result<SubmittedSession, Refusal> {
    let header = |name: &str| {
        req.headers()
            .get(name)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let stream_name = header(STREAM_NAME_HEADER);
    let stream_arn = header(STREAM_ARN_HEADER);
    // The stream is named by EXACTLY one of the two headers, as it is by exactly one of
    // the two members on the read plane: the two name one resource, so a request
    // carrying both says nothing about which to believe.
    match (&stream_name, &stream_arn) {
        (Some(_), None) | (None, Some(_)) => {}
        (Some(_), Some(_)) => {
            return Err(invalid_argument(format!(
                "Only one of {STREAM_NAME_HEADER} and {STREAM_ARN_HEADER} may be specified."
            )))
        }
        (None, None) => {
            return Err(invalid_argument(format!(
                "Either {STREAM_NAME_HEADER} or {STREAM_ARN_HEADER} must be specified."
            )))
        }
    }

    let Some(timecode_type) = header(TIMECODE_TYPE_HEADER) else {
        return Err(invalid_argument(format!(
            "{TIMECODE_TYPE_HEADER} must be specified."
        )));
    };
    let Some(timecode_type) = TimecodeType::parse(&timecode_type) else {
        return Err(invalid_argument(format!(
            "{TIMECODE_TYPE_HEADER} must be one of [{TIMECODE_TYPE_RELATIVE}, \
             {TIMECODE_TYPE_ABSOLUTE}]."
        )));
    };

    // The producer start timestamp is what a RELATIVE Cluster timecode is an offset FROM,
    // so a RELATIVE session cannot be read without it. An ABSOLUTE timecode already names
    // the instant its fragment was captured at, so nothing in such a session reads the
    // producer's start and a session that states none is taken as it stands.
    let producer_start_millis = match header(PRODUCER_START_TIMESTAMP_HEADER) {
        Some(producer_start) => Some(epoch_seconds_as_millis(&producer_start).ok_or_else(
            || {
                invalid_argument(format!(
                    "{PRODUCER_START_TIMESTAMP_HEADER} must be a timestamp in epoch seconds."
                ))
            },
        )?),
        None => match timecode_type {
            TimecodeType::Relative => {
                return Err(invalid_argument(format!(
                    "{PRODUCER_START_TIMESTAMP_HEADER} must be specified when \
                     {TIMECODE_TYPE_HEADER} is {TIMECODE_TYPE_RELATIVE}."
                )))
            }
            TimecodeType::Absolute => None,
        },
    };

    Ok(SubmittedSession {
        stream_name,
        stream_arn,
        timecode_type,
        producer_start_millis,
    })
}

/// A producer start timestamp, sent as epoch seconds (whole or fractional), in epoch
/// milliseconds — the precision Kinesis Video Streams records fragment timestamps at.
///
/// `None` for a value that is not a finite number of seconds, or one no instant can be
/// held at.
fn epoch_seconds_as_millis(value: &str) -> Option<i64> {
    let seconds: f64 = value.parse().ok()?;
    if !seconds.is_finite() {
        return None;
    }
    let millis = (seconds * 1000.0).round();
    // The bound is what a millisecond instant can be held in: beyond it the conversion
    // would be a different number than the producer sent.
    (millis >= -(2f64.powi(62)) && millis <= 2f64.powi(62)).then_some(millis as i64)
}

/// `InvalidArgumentException` (HTTP 400) stating `message`.
fn invalid_argument(message: String) -> Refusal {
    Refusal {
        status: 400,
        code: INVALID_ARGUMENT,
        message,
    }
}

/// Whether the ADDRESS a request arrived on is one the ingest route is served at.
///
/// A Kinesis Video Streams media API is served at the address announced for the media
/// endpoint that binds it — "Caller used wrong endpoint to write data to a stream. On
/// receiving such an exception, the user must call `GetDataEndpoint` with `AccessMode` set
/// to \"READ\"" — so the ingest route is bound on the address
/// `GetDataEndpoint(APIName=PUT_MEDIA)` hands a producer, which is this service's own
/// endpoint and so the address `GET_MEDIA` resolves to as well, and on no other address of
/// this emulator. A producer that skipped discovery, cached the control-plane endpoint or
/// addressed the archived-media endpoint has reached an endpoint that does not route
/// `/putMedia`, and is answered as such ([`unrouted_address_response`]) with nothing
/// ingested.
///
/// Which plane an address IS comes from the control plane that announced it
/// ([`control_plane::published_plane`]): this emulator knows the plane of the addresses it
/// published and of no others, so a request that reached a host NAMING a service — a
/// caller resolving AWS's own data hosts rather than an address this emulator handed out —
/// is left to the route's own claim.
fn ingest_served_at<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    let host = req.headers().get(::http::header::HOST.as_str());
    let Some(address) =
        ::service_common::data_plane_addressing::reached_emulator_endpoint(req.uri(), host)
    else {
        return true;
    };
    control_plane::published_plane(engine, &address) == Some(control_plane::DataPlane::Media)
}

/// What an address that does not route `/putMedia` answers a producer that sent one
/// there: HTTP 404, the answer an endpoint gives a path it binds NO ROUTE for
/// ([`control_plane_front_door::unrouted_path_response`]) — the bare unknown-operation
/// element as the whole body. Nothing is read from the request and nothing is ingested.
///
/// The ingest route is bound at ONE address of this emulator — the one
/// `GetDataEndpoint(APIName=PUT_MEDIA)` announces — so every other endpoint, the
/// control-plane front door and the address announced for the archived-media endpoint
/// among them, hosts no API at that path at all. That is a missing route rather than a
/// request the front door read and could name no operation in, which is why it is answered
/// neither with that envelope nor with its status.
pub fn unrouted_address_response() -> RtResponse {
    control_plane_front_door::unrouted_path_response()
}

/// Whether the buffered request parts address the ingest plane: a `POST` whose first URI
/// path segment is `putMedia`.
///
/// This is the CLAIM half of serving it, read from the same two things the answer is
/// ([`claims_request`]), so the claim and the answer cannot disagree about which
/// requests this plane serves. The route key is checked by the caller, which owns that
/// half of the rule for every one of this service's routes.
pub fn claims_parts(parts: &http::request::Parts) -> bool {
    parts.method == http::Method::POST && addresses_ingest_plane(&parts.uri.to_string())
}

/// Whether a runtime-API request addresses the ingest plane, by the same rule
/// [`claims_parts`] applies.
pub fn claims_request(req: &RtRequest) -> bool {
    req.method() == http::Method::POST.as_str() && addresses_ingest_plane(req.uri())
}

/// The one reading of that claim: the request's first URI path segment is the ingest
/// plane's.
fn addresses_ingest_plane(uri: &str) -> bool {
    ::service_common::nth_path_segment(::service_common::request_path(uri), 0)
        == Some(PUT_MEDIA_PATH_SEGMENT)
}
