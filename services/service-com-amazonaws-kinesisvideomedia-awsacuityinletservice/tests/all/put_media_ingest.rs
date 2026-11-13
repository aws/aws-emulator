// SPDX-License-Identifier: CC0-1.0

//! The ingest plane end to end: `POST /putMedia` to a created stream, then the same
//! stream read back through both of its read planes.
//!
//! The rule under test: a Kinesis Video Streams ingest endpoint takes a Matroska
//! container submitted to `POST /putMedia` for the stream its `x-amzn-stream-name` /
//! `x-amzn-stream-arn` header names, persists ONE fragment per Cluster — at the instant
//! the Cluster's timecode names under the session's `x-amzn-fragment-timecode-type` —
//! and answers `200` whose body is the acknowledgement stream for that session:
//! `BUFFERING`, `RECEIVED` and `PERSISTED` per accepted Cluster, each carrying the
//! fragment number the service minted and that Cluster's own timecode. Those fragments
//! are then the stream's media on every read plane: `GetMedia` streams the submitted
//! frames back and `ListFragments` reports exactly the ingested fragments. A fault in
//! the REQUEST is an HTTP error in this service's modelled vocabulary; a fault in the
//! CONTAINER is reported in band, inside the same `200`, and the Clusters accepted
//! before it stay ingested.
//!
//! The tables span what the rule generalises over: how many Clusters one session carries
//! and how many frames each one holds, both timecode types, both stream identifier
//! forms, a segment of known and of unknown size, the container's own timecode scale,
//! and the frame bytes themselves. The three-Cluster `RELATIVE` session the issue
//! recorded is one row.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as control_plane;
use service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader as archived_media;
use service_com_amazonaws_kinesisvideomedia_awsacuityinletservice as media;
use smithy_server_runtime::{MultiServiceRouter, Service};

use crate::get_media::{read_ebml_element, read_kvs_tags, split_payload};
use media::ops::get_media::{push_ebml_element, ID_TAGS};

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The host a `GetDataEndpoint` answer points a data-plane call at in this suite.
const ENDPOINT_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

// ---------------------------------------------------------------------------
// The Matroska container a producer submits.
// ---------------------------------------------------------------------------
//
// Written here, in the test, because the emulator only READS this container: the element
// ids are the Matroska specification's and the sizes are EBML variable-size integers
// (RFC 8794 section 4), written through the service crate's own element writer so the
// bytes under test are not produced by the code under test.

/// Matroska `EBML` — the document's header (level 0).
const ID_EBML: &[u8] = &[0x1A, 0x45, 0xDF, 0xA3];
/// Matroska `DocType`.
const ID_DOC_TYPE: &[u8] = &[0x42, 0x82];
/// Matroska `Segment` (level 0), the master element the media sits in.
const ID_SEGMENT: &[u8] = &[0x18, 0x53, 0x80, 0x67];
/// Matroska `Info`.
const ID_INFO: &[u8] = &[0x15, 0x49, 0xA9, 0x66];
/// Matroska `TimestampScale` (nanoseconds per timecode tick).
const ID_TIMESTAMP_SCALE: &[u8] = &[0x2A, 0xD7, 0xB1];
/// Matroska `Tracks`.
const ID_TRACKS: &[u8] = &[0x16, 0x54, 0xAE, 0x6B];
/// Matroska `TrackEntry`.
const ID_TRACK_ENTRY: &[u8] = &[0xAE];
/// Matroska `TrackNumber`.
const ID_TRACK_NUMBER: &[u8] = &[0xD7];
/// Matroska `TrackType`.
const ID_TRACK_TYPE: &[u8] = &[0x83];
/// Matroska `CodecID`.
const ID_CODEC_ID: &[u8] = &[0x86];
/// Matroska `Cluster`.
const ID_CLUSTER: &[u8] = &[0x1F, 0x43, 0xB6, 0x75];
/// Matroska `Timestamp` (a Cluster's own timecode).
const ID_TIMESTAMP: &[u8] = &[0xE7];
/// Matroska `SimpleBlock` (one frame).
const ID_SIMPLE_BLOCK: &[u8] = &[0xA3];

/// The EBML "unknown size" encoding: an eight-byte variable-size integer whose data bits
/// are all ones (RFC 8794 section 4.6), which is what a live producer writes for a
/// segment whose end has not happened yet.
const UNKNOWN_SIZE: &[u8] = &[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// The default Matroska timecode scale: one millisecond per tick, in nanoseconds.
const MILLISECOND_SCALE_NANOS: u64 = 1_000_000;

/// The track number every frame in this suite is written on, declared in `Tracks`.
const TRACK_NUMBER: u64 = 1;

/// The codec the declared track carries — the one the Kinesis Video Streams producers
/// send for H.264.
const CODEC_ID: &str = "V_MPEG4/ISO/AVC";

/// `value` as a Matroska unsigned-integer element body: big-endian, shortest form.
fn uint_body(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let first = bytes.iter().position(|byte| *byte != 0).unwrap_or(7);
    bytes[first..].to_vec()
}

/// One submitted Cluster: its timecode, and the frames it carries with their offsets
/// from it.
#[derive(Debug, Clone)]
pub(crate) struct Cluster {
    /// The Cluster's `Timestamp`, in the container's timecode scale.
    timecode: u64,
    /// The track number every `SimpleBlock` of this Cluster is written on.
    track: u64,
    /// `(offset from the Cluster timecode, frame bytes)` per frame.
    frames: Vec<(i16, Vec<u8>)>,
}

/// One submitted container.
#[derive(Debug, Clone)]
pub(crate) struct Container {
    /// Nanoseconds per timecode tick, as `Info/TimestampScale` declares it.
    timecode_scale_nanos: u64,
    /// Whether the `Segment` is written with an unknown size, as a live producer writes
    /// it.
    unknown_size_segment: bool,
    clusters: Vec<Cluster>,
}

impl Cluster {
    /// The frames this Cluster carries, in order — the bytes a reader of the stream has
    /// to get back.
    pub(crate) fn frames(&self) -> Vec<Vec<u8>> {
        self.frames.iter().map(|(_, frame)| frame.clone()).collect()
    }
}

impl Container {
    /// A container of `clusters` with a millisecond timecode scale and a sized segment.
    pub(crate) fn of(clusters: Vec<Cluster>) -> Self {
        Self {
            timecode_scale_nanos: MILLISECOND_SCALE_NANOS,
            unknown_size_segment: false,
            clusters,
        }
    }

    /// The container's bytes.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        let mut document = Vec::new();
        let mut header = Vec::new();
        push_ebml_element(&mut header, ID_DOC_TYPE, b"matroska");
        push_ebml_element(&mut document, ID_EBML, &header);

        let mut segment = Vec::new();
        let mut info = Vec::new();
        push_ebml_element(
            &mut info,
            ID_TIMESTAMP_SCALE,
            &uint_body(self.timecode_scale_nanos),
        );
        push_ebml_element(&mut segment, ID_INFO, &info);

        let mut track = Vec::new();
        push_ebml_element(&mut track, ID_TRACK_NUMBER, &uint_body(TRACK_NUMBER));
        // `TrackType` 1 is video (Matroska `TrackType` value set).
        push_ebml_element(&mut track, ID_TRACK_TYPE, &uint_body(1));
        push_ebml_element(&mut track, ID_CODEC_ID, CODEC_ID.as_bytes());
        let mut tracks = Vec::new();
        push_ebml_element(&mut tracks, ID_TRACK_ENTRY, &track);
        push_ebml_element(&mut segment, ID_TRACKS, &tracks);

        for cluster in &self.clusters {
            let mut body = Vec::new();
            push_ebml_element(&mut body, ID_TIMESTAMP, &uint_body(cluster.timecode));
            for (offset, frame) in &cluster.frames {
                // A `SimpleBlock` body is the track number as a variable-size integer,
                // the frame's signed 16-bit offset from the Cluster timecode, one flags
                // byte, then the frame itself.
                let mut block = vec![0x80 | (cluster.track as u8)];
                block.extend_from_slice(&offset.to_be_bytes());
                block.push(0x80);
                block.extend_from_slice(frame);
                push_ebml_element(&mut body, ID_SIMPLE_BLOCK, &block);
            }
            push_ebml_element(&mut segment, ID_CLUSTER, &body);
        }

        if self.unknown_size_segment {
            document.extend_from_slice(ID_SEGMENT);
            document.extend_from_slice(UNKNOWN_SIZE);
            document.extend_from_slice(&segment);
        } else {
            push_ebml_element(&mut document, ID_SEGMENT, &segment);
        }
        document
    }
}

/// A Cluster at `timecode` carrying `frames` frames of `spacing` milliseconds, whose
/// bytes carry `marker` so one Cluster's frames cannot be mistaken for another's. Its
/// frames are written on the track the container declares.
pub(crate) fn cluster(timecode: u64, frames: usize, spacing: i16, marker: u8) -> Cluster {
    cluster_on_track(timecode, TRACK_NUMBER, frames, spacing, marker)
}

/// [`cluster`] with its frames written on `track`, so a case can submit a block on a
/// track the container's `Tracks` does not declare.
pub(crate) fn cluster_on_track(
    timecode: u64,
    track: u64,
    frames: usize,
    spacing: i16,
    marker: u8,
) -> Cluster {
    Cluster {
        timecode,
        track,
        frames: (0..frames)
            .map(|index| {
                let offset = spacing * (index as i16);
                let frame = vec![
                    marker,
                    index as u8,
                    0xFF,
                    marker ^ 0x5A,
                    (timecode & 0xFF) as u8,
                ];
                (offset, frame)
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// The harness.
// ---------------------------------------------------------------------------

/// One router over one engine carrying the three services a round trip needs: the
/// control plane that creates the stream, this ingest/media service, and the archive
/// read plane that reports the ingested fragments.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(control_plane::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(media::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(archived_media::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The SigV4 credential scope every request in this suite carries: its service field
    /// is the routing key the five Kinesis Video services share.
    fn authorization() -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261031/{REGION}/kinesisvideo/\
             aws4_request, SignedHeaders=host, Signature=0000"
        )
    }

    /// POST one restJson1 call at the data endpoint host.
    async fn call(&self, path: &str, body: &serde_json::Value) -> http::Response<Bytes> {
        self.call_at(ENDPOINT_HOST, path, body).await
    }

    /// POST one restJson1 call at `authority` — the address the request arrives on.
    async fn call_at(
        &self,
        authority: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> http::Response<Bytes> {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{authority}{path}"))
            .header("host", authority)
            .header("content-type", "application/json")
            .header("authorization", Self::authorization())
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.router.dispatch(request).await
    }

    /// POST one ingest session: the headers a producer sends and the container as its
    /// body.
    async fn put_media(
        &self,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> http::Response<Bytes> {
        self.put_media_at(&format!("https://{ENDPOINT_HOST}/putMedia"), headers, body)
            .await
    }

    /// POST one ingest session to `url` — the address and path a producer builds from
    /// the endpoint `GetDataEndpoint` handed it.
    async fn put_media_at(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> http::Response<Bytes> {
        let authority = url
            .split_once("://")
            .and_then(|(_, rest)| rest.split('/').next())
            .unwrap_or_default()
            .to_string();
        let mut request = http::Request::builder()
            .method("POST")
            .uri(url)
            .header("host", authority)
            .header("authorization", Self::authorization());
        for (name, value) in headers {
            request = request.header(*name, value.clone());
        }
        self.router
            .dispatch(
                request
                    .body(Bytes::from(body))
                    .expect("a well-formed request"),
            )
            .await
    }

    /// Create `name` through the control plane, wait for it to be readable, and answer
    /// the `StreamARN` it minted.
    async fn create_stream(&self, name: &str) -> String {
        let response = self
            .call(
                "/createStream",
                &serde_json::json!({ "StreamName": name, "DataRetentionInHours": 1 }),
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "case \"create {name}\": expected HTTP 200, got {} {}",
            status(&response),
            text(&response)
        );
        let arn = json(&response)
            .get("StreamARN")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case \"create {name}\": no StreamARN in the answer"))
            .to_string();
        self.await_active(name).await;
        arn
    }

    /// Wait until the control plane reports `name` as `ACTIVE`, as a client does before
    /// reading a stream it just created ("CreateStream is an asynchronous operation"):
    /// the archive read plane serves a stream only while the registry holds it active.
    ///
    /// Waits for the CONDITION, not for a span: it looks again until the status holds,
    /// under a deadline far longer than the transition needs.
    async fn await_active(&self, name: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut last = String::new();
        while std::time::Instant::now() < deadline {
            let response = self
                .call("/describeStream", &serde_json::json!({ "StreamName": name }))
                .await;
            last = json(&response)["StreamInfo"]["Status"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if last == "ACTIVE" {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("case \"create {name}\": expected Status ACTIVE before the deadline, got {last:?}");
    }

    /// The chunks this service's media log holds for the stream `arn` names, in order.
    fn stored_chunks(&self, arn: &str) -> Vec<Vec<u8>> {
        let state = media::state::service_state(&*self.engine).expect("the media store");
        let store = state.lock();
        store
            .log_by_arn(arn)
            .map(|log| {
                log.fragments
                    .iter()
                    .map(|fragment| fragment.chunk.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `ListFragments` over a window wide enough to hold every fragment this suite
    /// ingests, as `(fragment number, length in milliseconds)` pairs.
    async fn list_fragments(&self, addressing: &serde_json::Value) -> Vec<(String, i64)> {
        let mut body = addressing.as_object().cloned().unwrap_or_default();
        body.insert(
            "FragmentSelector".to_string(),
            serde_json::json!({
                "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                "TimestampRange": { "StartTimestamp": 0, "EndTimestamp": 4_000_000_000i64 },
            }),
        );
        let response = self
            .call("/listFragments", &serde_json::Value::Object(body))
            .await;
        assert_eq!(
            status(&response),
            200,
            "case \"listFragments\": expected HTTP 200, got {} {}",
            status(&response),
            text(&response)
        );
        json(&response)
            .get("Fragments")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|fragment| {
                (
                    fragment
                        .get("FragmentNumber")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    fragment
                        .get("FragmentLengthInMilliseconds")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or_default(),
                )
            })
            .collect()
    }

    /// `GetMedia` from the earliest chunk of the stream `addressing` names.
    async fn get_media_earliest(&self, addressing: &serde_json::Value) -> http::Response<Bytes> {
        let mut body = addressing.as_object().cloned().unwrap_or_default();
        body.insert(
            "StartSelector".to_string(),
            serde_json::json!({ "StartSelectorType": "EARLIEST" }),
        );
        self.call("/getMedia", &serde_json::Value::Object(body)).await
    }
}

fn status(response: &http::Response<Bytes>) -> u16 {
    response.status().as_u16()
}

fn error_code(response: &http::Response<Bytes>) -> String {
    response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn json(response: &http::Response<Bytes>) -> serde_json::Value {
    serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
}

fn text(response: &http::Response<Bytes>) -> String {
    String::from_utf8_lossy(response.body()).to_string()
}

/// The acknowledgements an ingest answer carries, in order.
///
/// The body is a STREAM of acknowledgements: one JSON object each, whose members are the
/// acknowledgement's own (`EventType`, `FragmentTimecode`, `FragmentNumber`, and
/// `ErrorCode`/`ErrorId` on a fault), which is what a producer reads off this wire.
fn acknowledgements(case: &str, response: &http::Response<Bytes>) -> Vec<serde_json::Value> {
    text(response)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("case {case:?}: an acknowledgement is not JSON ({error}): {line:?}")
            });
            assert!(
                value.get("EventType").and_then(serde_json::Value::as_str).is_some(),
                "case {case:?}: an acknowledgement states its own EventType, got {line:?}"
            );
            value
        })
        .collect()
}

/// One acknowledgement's `EventType`.
fn event_type(acknowledgement: &serde_json::Value) -> String {
    acknowledgement
        .get("EventType")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The fragment numbers the `PERSISTED` acknowledgements carry, with the Cluster
/// timecode each one reports.
fn persisted(acknowledgements: &[serde_json::Value]) -> Vec<(String, u64)> {
    acknowledgements
        .iter()
        .filter(|acknowledgement| event_type(acknowledgement) == "PERSISTED")
        .map(|acknowledgement| {
            (
                acknowledgement
                    .get("FragmentNumber")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                acknowledgement
                    .get("FragmentTimecode")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// The fragment numbers a `GetMedia` payload delivered, read out of the
/// `AWS_KINESISVIDEO_*` tags the response frames each chunk with.
fn delivered_fragment_numbers(case: &str, payload: &[u8], chunks: &[Vec<u8>]) -> Vec<String> {
    let borrowed: Vec<&[u8]> = chunks.iter().map(Vec::as_slice).collect();
    split_payload(case, payload, &borrowed)
        .iter()
        .map(|chunk| chunk.tag("AWS_KINESISVIDEO_FRAGMENT_NUMBER"))
        .collect()
}

/// The ingest headers of one request: the `(name, value)` pairs a producer sends.
pub(crate) type IngestHeaders = Vec<(&'static str, String)>;

/// The ingest headers of one session.
pub(crate) fn ingest_headers(
    addressing: (&'static str, String),
    timecode_type: &str,
    producer_start: SystemTime,
) -> IngestHeaders {
    let seconds = producer_start
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
        / 1000.0;
    vec![
        addressing,
        ("x-amzn-fragment-timecode-type", timecode_type.to_string()),
        ("x-amzn-producer-start-timestamp", format!("{seconds:.3}")),
        // A producer SIGNS its media, and this is the header the signature covers the
        // body through: what it sends for an unsigned-length stream.
        ("x-amz-content-sha256", "UNSIGNED-PAYLOAD".to_string()),
    ]
}

/// Epoch milliseconds of `at`.
fn epoch_millis(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// One ingest case: the container, how it is addressed and how its timecodes are read.
struct IngestCase {
    name: &'static str,
    container: Container,
    /// `true` to address the stream by its ARN, `false` by its name.
    by_arn: bool,
    timecode_type: &'static str,
    /// The producer timestamps the fragments must land on, in epoch milliseconds.
    expected_producer_millis: Vec<u64>,
    /// The fragment lengths the archive must report, in milliseconds.
    expected_lengths: Vec<i64>,
}

/// RULE: one ingest session persists one fragment per submitted Cluster and is
/// acknowledged fragment by fragment, and those fragments are then the stream's media on
/// both of its read planes.
#[tokio::test]
async fn an_ingest_session_persists_one_fragment_per_cluster_and_is_read_back() {
    // The producer start timestamp of every RELATIVE session: a whole second, eight
    // seconds back, as a producer's first fragment is stamped.
    let producer_start = SystemTime::UNIX_EPOCH + Duration::from_millis(1_761_000_000_000);
    let start_millis = epoch_millis(producer_start);

    let cases: Vec<IngestCase> = vec![
        IngestCase {
            // The session the issue recorded: three Clusters a second apart, three
            // frames each on a declared track.
            name: "three RELATIVE clusters addressed by name",
            container: Container::of(vec![
                cluster(0, 3, 33, 0xA1),
                cluster(1000, 3, 33, 0xB2),
                cluster(2000, 3, 33, 0xC3),
            ]),
            by_arn: false,
            timecode_type: "RELATIVE",
            expected_producer_millis: vec![start_millis, start_millis + 1000, start_millis + 2000],
            expected_lengths: vec![66, 66, 66],
        },
        IngestCase {
            name: "one RELATIVE cluster of one frame addressed by ARN, unknown-size segment",
            container: Container {
                timecode_scale_nanos: MILLISECOND_SCALE_NANOS,
                unknown_size_segment: true,
                clusters: vec![cluster(500, 1, 0, 0xD4)],
            },
            by_arn: true,
            timecode_type: "RELATIVE",
            expected_producer_millis: vec![start_millis + 500],
            expected_lengths: vec![0],
        },
        IngestCase {
            // ABSOLUTE: the Cluster timecode IS the epoch instant, so the producer start
            // timestamp does not move the fragment.
            name: "two ABSOLUTE clusters addressed by name",
            container: Container::of(vec![
                cluster(1_700_000_000_000, 2, 40, 0xE5),
                cluster(1_700_000_004_000, 5, 20, 0xF6),
            ]),
            by_arn: false,
            timecode_type: "ABSOLUTE",
            expected_producer_millis: vec![1_700_000_000_000, 1_700_000_004_000],
            expected_lengths: vec![40, 80],
        },
        IngestCase {
            // A timecode scale of 100 microseconds: ten ticks to the millisecond, so
            // the fragment lands at a tenth of its raw timecode.
            name: "two RELATIVE clusters at a 100-microsecond timecode scale",
            container: Container {
                timecode_scale_nanos: 100_000,
                unknown_size_segment: false,
                clusters: vec![cluster(0, 2, 100, 0x17), cluster(20_000, 2, 100, 0x28)],
            },
            by_arn: true,
            timecode_type: "RELATIVE",
            expected_producer_millis: vec![start_millis, start_millis + 2000],
            expected_lengths: vec![10, 10],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let harness = Harness::new();
        let name = format!("ingest-probe-{index}");
        let arn = harness.create_stream(&name).await;
        let case_name = case.name;

        let addressing = if case.by_arn {
            serde_json::json!({ "StreamARN": arn })
        } else {
            serde_json::json!({ "StreamName": name })
        };
        let header = if case.by_arn {
            ("x-amzn-stream-arn", arn.clone())
        } else {
            ("x-amzn-stream-name", name.clone())
        };

        // Before ingestion the archive is empty — the half of both recorded checks that
        // already passed.
        let before = harness.list_fragments(&addressing).await;
        assert!(
            before.is_empty(),
            "case {case_name:?}: a created stream holds no fragments, got {before:?}"
        );

        let response = harness
            .put_media(
                &ingest_headers(header, case.timecode_type, producer_start),
                case.container.bytes(),
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "case {case_name:?}: expected HTTP 200 from the ingest plane, got {} ({}) {}",
            status(&response),
            error_code(&response),
            text(&response)
        );

        // The acknowledgement stream: every accepted Cluster's receipt, in submission
        // order, then every accepted Cluster's storing — the order the session's events
        // happen in — and no ERROR acknowledgement.
        let acks = acknowledgements(case_name, &response);
        let events: Vec<String> = acks.iter().map(event_type).collect();
        let mut expected_events: Vec<String> = case
            .container
            .clusters
            .iter()
            .flat_map(|_| ["BUFFERING", "RECEIVED"])
            .map(str::to_string)
            .collect();
        expected_events.extend(
            case.container
                .clusters
                .iter()
                .map(|_| "PERSISTED".to_string()),
        );
        assert_eq!(
            events, expected_events,
            "case {case_name:?}: expected the acknowledgement events {expected_events:?}, \
             got {events:?} in {}",
            text(&response)
        );

        let acknowledged = persisted(&acks);
        let timecodes: Vec<u64> = acknowledged.iter().map(|(_, at)| *at).collect();
        let submitted: Vec<u64> = case
            .container
            .clusters
            .iter()
            .map(|cluster| cluster.timecode)
            .collect();
        assert_eq!(
            timecodes, submitted,
            "case {case_name:?}: a PERSISTED acknowledgement must carry its Cluster's own \
             timecode, expected {submitted:?}, got {timecodes:?}"
        );
        let numbers: Vec<String> = acknowledged.iter().map(|(number, _)| number.clone()).collect();
        assert!(
            numbers.iter().all(|number| !number.is_empty()
                && number.chars().all(|digit| digit.is_ascii_digit())),
            "case {case_name:?}: every acknowledged fragment number must be a decimal \
             number, got {numbers:?}"
        );
        let mut distinct = numbers.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            numbers.len(),
            "case {case_name:?}: each accepted Cluster must get its own fragment number, \
             got {numbers:?}"
        );

        // The archive read plane reports exactly the ingested fragments, with the span
        // the submitted frames cover.
        let listed = harness.list_fragments(&addressing).await;
        let listed_numbers: Vec<String> = listed.iter().map(|(number, _)| number.clone()).collect();
        assert_eq!(
            listed_numbers, numbers,
            "case {case_name:?}: ListFragments must report the ingested fragments \
             {numbers:?}, got {listed:?}"
        );
        let lengths: Vec<i64> = listed.iter().map(|(_, length)| *length).collect();
        assert_eq!(
            lengths, case.expected_lengths,
            "case {case_name:?}: expected the fragment lengths {:?}, got {lengths:?}",
            case.expected_lengths
        );

        // The media read plane streams the stored chunks back, each framed with its own
        // fragment number, and the submitted frames survive the round trip byte for
        // byte.
        let chunks = harness.stored_chunks(&arn);
        assert_eq!(
            chunks.len(),
            case.container.clusters.len(),
            "case {case_name:?}: expected one stored chunk per Cluster, got {}",
            chunks.len()
        );
        for (at, submitted_cluster) in case.container.clusters.iter().enumerate() {
            let chunk = &chunks[at];
            for (_, frame) in &submitted_cluster.frames {
                assert!(
                    chunk
                        .windows(frame.len())
                        .any(|window| window == frame.as_slice()),
                    "case {case_name:?}: the chunk stored for the Cluster at {} must carry \
                     the submitted frame {frame:?}, got {chunk:?}",
                    submitted_cluster.timecode
                );
            }
        }
        let read = harness.get_media_earliest(&addressing).await;
        assert_eq!(
            status(&read),
            200,
            "case {case_name:?}: expected HTTP 200 from GetMedia, got {} ({})",
            status(&read),
            error_code(&read)
        );
        let delivered = delivered_fragment_numbers(case_name, read.body(), &chunks);
        assert_eq!(
            delivered, numbers,
            "case {case_name:?}: GetMedia must deliver the ingested fragments {numbers:?}, \
             got {delivered:?}"
        );

        // The producer timestamps the session's timecode type implies.
        let stamped = producer_timestamps(&harness, &arn);
        assert_eq!(
            stamped, case.expected_producer_millis,
            "case {case_name:?}: expected the producer timestamps {:?}, got {stamped:?}",
            case.expected_producer_millis
        );
    }
}

/// The producer timestamps the media log holds for the stream `arn` names, in epoch
/// milliseconds.
fn producer_timestamps(harness: &Harness, arn: &str) -> Vec<u64> {
    let state = media::state::service_state(&*harness.engine).expect("the media store");
    let store = state.lock();
    store
        .log_by_arn(arn)
        .map(|log| {
            log.fragments
                .iter()
                .map(|fragment| epoch_millis(fragment.producer_timestamp))
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: successive sessions on one stream EXTEND it — a later session's fragments get
/// later fragment numbers and the read planes report the whole archive, not the last
/// session's part of it.
#[tokio::test]
async fn successive_sessions_extend_one_streams_archive() {
    let harness = Harness::new();
    let name = "ingest-probe-sessions";
    let arn = harness.create_stream(name).await;
    let producer_start = SystemTime::UNIX_EPOCH + Duration::from_millis(1_761_000_100_000);

    let mut acknowledged: Vec<String> = Vec::new();
    for (session, timecodes) in [(0usize, vec![0u64, 1000]), (1, vec![2000, 3000, 4000])] {
        let container = Container::of(
            timecodes
                .iter()
                .enumerate()
                .map(|(at, timecode)| cluster(*timecode, 2, 25, (session * 16 + at) as u8))
                .collect(),
        );
        let response = harness
            .put_media(
                &ingest_headers(
                    ("x-amzn-stream-name", name.to_string()),
                    "RELATIVE",
                    producer_start,
                ),
                container.bytes(),
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "case \"session {session}\": expected HTTP 200, got {} {}",
            status(&response),
            text(&response)
        );
        let numbers: Vec<String> = persisted(&acknowledgements("sessions", &response))
            .iter()
            .map(|(number, _)| number.clone())
            .collect();
        assert_eq!(
            numbers.len(),
            timecodes.len(),
            "case \"session {session}\": expected one PERSISTED acknowledgement per \
             Cluster, got {numbers:?}"
        );
        acknowledged.extend(numbers);
    }

    let listed: Vec<String> = harness
        .list_fragments(&serde_json::json!({ "StreamName": name }))
        .await
        .iter()
        .map(|(number, _)| number.clone())
        .collect();
    assert_eq!(
        listed, acknowledged,
        "case \"two sessions\": the archive must hold both sessions' fragments \
         {acknowledged:?}, got {listed:?}"
    );
    let chunks = harness.stored_chunks(&arn);
    assert_eq!(
        chunks.len(),
        acknowledged.len(),
        "case \"two sessions\": the media log must hold both sessions' chunks, got {}",
        chunks.len()
    );
}

/// RULE: the producer start timestamp is what a RELATIVE Cluster timecode is an offset
/// FROM, so a RELATIVE session cannot be read without it — while an ABSOLUTE Cluster
/// timecode already names the instant its fragment was captured at, so a session that
/// states none is ingested as it stands and its fragments land at the instants its
/// timecodes name.
///
/// The rows differ in what the session declares: ABSOLUTE with the header and without it
/// (the same fragments either way, since nothing in such a session reads it), and RELATIVE
/// without it, which is the one refusal.
#[tokio::test]
async fn the_producer_start_timestamp_is_read_only_by_a_relative_session() {
    // Epoch milliseconds the absolute Clusters name — in the past, as a producer's own
    // capture instants are.
    const FIRST_MILLIS: u64 = 1_761_000_000_000;
    const SECOND_MILLIS: u64 = 1_761_000_002_000;

    // (case, the timecode type, whether the session states a producer start timestamp,
    // the expected status, the producer timestamps the fragments must land on).
    let rows: Vec<(&str, &str, bool, u16, Vec<u64>)> = vec![
        (
            "ABSOLUTE stating a producer start timestamp",
            "ABSOLUTE",
            true,
            200,
            vec![FIRST_MILLIS, SECOND_MILLIS],
        ),
        (
            "ABSOLUTE stating none",
            "ABSOLUTE",
            false,
            200,
            vec![FIRST_MILLIS, SECOND_MILLIS],
        ),
        ("RELATIVE stating none", "RELATIVE", false, 400, Vec::new()),
    ];

    for (index, (case, timecode_type, states_start, expected_status, expected_millis)) in
        rows.into_iter().enumerate()
    {
        let harness = Harness::new();
        let name = format!("ingest-probe-start-{index}");
        let arn = harness.create_stream(&name).await;
        let producer_start = SystemTime::UNIX_EPOCH + Duration::from_millis(FIRST_MILLIS);
        let headers = {
            let stated = ingest_headers(
                ("x-amzn-stream-name", name.clone()),
                timecode_type,
                producer_start,
            );
            if states_start {
                stated
            } else {
                stated
                    .into_iter()
                    .filter(|(header, _)| *header != "x-amzn-producer-start-timestamp")
                    .collect()
            }
        };
        let container = Container::of(vec![
            cluster(FIRST_MILLIS, 2, 30, 0x66),
            cluster(SECOND_MILLIS, 1, 0, 0x67),
        ]);
        let response = harness.put_media(&headers, container.bytes()).await;
        assert_eq!(
            status(&response),
            expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} ({}) {}",
            status(&response),
            error_code(&response),
            text(&response)
        );
        let landed = producer_timestamps(&harness, &arn);
        assert_eq!(
            landed, expected_millis,
            "case {case:?}: expected the fragments at {expected_millis:?}, got {landed:?}"
        );
    }
}

/// RULE: a fault in the REQUEST is answered as an HTTP error in this service's modelled
/// error vocabulary, and nothing is ingested.
#[tokio::test]
async fn a_request_level_fault_is_refused_and_ingests_nothing() {
    let harness = Harness::new();
    let name = "ingest-probe-refusals";
    let arn = harness.create_stream(name).await;
    let container = Container::of(vec![cluster(0, 2, 30, 0x3C)]);
    let producer_start = SystemTime::UNIX_EPOCH + Duration::from_millis(1_761_000_200_000);
    let valid = ingest_headers(
        ("x-amzn-stream-name", name.to_string()),
        "RELATIVE",
        producer_start,
    );

    // (case, headers, expected status, expected error code)
    let rows: Vec<(&str, IngestHeaders, u16, &str)> = vec![
        (
            "no stream identifier at all",
            valid[1..].to_vec(),
            400,
            "InvalidArgumentException",
        ),
        (
            "both stream identifiers",
            {
                let mut headers = valid.clone();
                headers.push(("x-amzn-stream-arn", arn.clone()));
                headers
            },
            400,
            "InvalidArgumentException",
        ),
        (
            "no fragment timecode type",
            vec![
                ("x-amzn-stream-name", name.to_string()),
                valid[2].clone(),
                valid[3].clone(),
            ],
            400,
            "InvalidArgumentException",
        ),
        (
            "a fragment timecode type outside the published set",
            ingest_headers(
                ("x-amzn-stream-name", name.to_string()),
                "WALLCLOCK",
                producer_start,
            ),
            400,
            "InvalidArgumentException",
        ),
        (
            "a producer start timestamp that is not a timestamp",
            vec![
                ("x-amzn-stream-name", name.to_string()),
                ("x-amzn-fragment-timecode-type", "RELATIVE".to_string()),
                ("x-amzn-producer-start-timestamp", "yesterday".to_string()),
                valid[3].clone(),
            ],
            400,
            "InvalidArgumentException",
        ),
        (
            "a stream name nothing created",
            ingest_headers(
                ("x-amzn-stream-name", "ingest-probe-never-created".to_string()),
                "RELATIVE",
                producer_start,
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a well-formed ARN the registry never held",
            ingest_headers(
                (
                    "x-amzn-stream-arn",
                    arn.replace(name, "ingest-probe-never-created"),
                ),
                "RELATIVE",
                producer_start,
            ),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, headers, expected_status, expected_code) in rows {
        let response = harness.put_media(&headers, container.bytes()).await;
        assert_eq!(
            status(&response),
            expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} ({}) {}",
            status(&response),
            error_code(&response),
            text(&response)
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: expected the error code {expected_code}, got {:?}",
            error_code(&response)
        );
        assert!(
            json(&response).get("Message").is_some(),
            "case {case:?}: a refusal states its Message, got {}",
            text(&response)
        );
    }

    // Nothing was ingested by any of the refused requests.
    let listed = harness
        .list_fragments(&serde_json::json!({ "StreamName": name }))
        .await;
    assert!(
        listed.is_empty(),
        "case \"after the refusals\": no fragment may have been ingested, got {listed:?}"
    );
    assert!(
        harness.stored_chunks(&arn).is_empty(),
        "case \"after the refusals\": the media log must be empty"
    );

    // The sentence a missing stream is refused with is this FAMILY's: the one its
    // control plane answers every missing-stream read with, so one condition reads one
    // way whichever plane reports it.
    let refused = harness
        .put_media(
            &ingest_headers(
                ("x-amzn-stream-name", "ingest-probe-never-created".to_string()),
                "RELATIVE",
                producer_start,
            ),
            container.bytes(),
        )
        .await;
    assert_eq!(
        json(&refused).get("Message").and_then(serde_json::Value::as_str),
        Some(control_plane::state::STREAM_NOT_FOUND_MESSAGE),
        "case \"a stream the registry does not hold\": expected the family's own sentence, \
         got {}",
        text(&refused)
    );
}

/// RULE: a session's producer start timestamp is what a RELATIVE Cluster timecode is an
/// offset FROM, so an ABSOLUTE session — whose timecodes already name their instants — is
/// taken without one, and a timestamp AHEAD of this service's clock by more than two
/// clocks can differ by is a fault of the session's timing, reported in band, for which
/// nothing is taken.
///
/// The rows differ in how the session is dated: not at all (ABSOLUTE), in the past, and
/// well beyond the allowance in the future.
#[tokio::test]
async fn a_sessions_producer_start_timestamp_is_optional_and_may_not_be_in_the_future() {
    let now = SystemTime::now();
    // (case, the start timestamp the session states, its timecode type, whether it is
    // taken)
    let rows: Vec<(&str, Option<SystemTime>, &str, bool)> = vec![
        ("no start timestamp at all", None, "ABSOLUTE", true),
        (
            "a minute in the past",
            Some(now - Duration::from_secs(60)),
            "RELATIVE",
            true,
        ),
        (
            "a day in the future",
            Some(now + Duration::from_secs(86_400)),
            "RELATIVE",
            false,
        ),
        (
            "a year in the future",
            Some(now + Duration::from_secs(365 * 86_400)),
            "RELATIVE",
            false,
        ),
    ];

    for (index, (case, start, timecode_type, taken)) in rows.into_iter().enumerate() {
        let harness = Harness::new();
        let name = format!("ingest-probe-dated-{index}");
        let arn = harness.create_stream(&name).await;
        let headers: IngestHeaders = match start {
            Some(start) => ingest_headers(
                ("x-amzn-stream-name", name.clone()),
                timecode_type,
                start,
            ),
            // Everything a producer sends EXCEPT the start timestamp.
            None => vec![
                ("x-amzn-stream-name", name.clone()),
                ("x-amzn-fragment-timecode-type", timecode_type.to_string()),
                ("x-amz-content-sha256", "UNSIGNED-PAYLOAD".to_string()),
            ],
        };
        let container = Container::of(vec![cluster(0, 2, 20, 0x61)]);
        let response = harness.put_media(&headers, container.bytes()).await;
        assert_eq!(
            status(&response),
            200,
            "case {case:?}: a session's own timing is reported in band, so the status \
             stays 200, got {} ({}) {}",
            status(&response),
            error_code(&response),
            text(&response)
        );
        let acks = acknowledgements(case, &response);
        let errors: Vec<(String, u64)> = acks
            .iter()
            .filter(|ack| event_type(ack) == "ERROR")
            .map(|ack| {
                (
                    ack.get("ErrorCode")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    ack.get("ErrorId")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or_default(),
                )
            })
            .collect();
        if taken {
            assert!(
                errors.is_empty(),
                "case {case:?}: a session dated at or before this service's clock is \
                 taken, got {}",
                text(&response)
            );
            assert_eq!(
                persisted(&acks).len(),
                1,
                "case {case:?}: the session's one Cluster is acknowledged persisted, got {}",
                text(&response)
            );
            assert_eq!(
                harness.stored_chunks(&arn).len(),
                1,
                "case {case:?}: the stream holds the fragment the session produced"
            );
            continue;
        }
        assert_eq!(
            errors,
            vec![("INVALID_PRODUCER_TIMESTAMP".to_string(), 4007)],
            "case {case:?}: expected one ERROR acknowledgement of the session's timing, \
             got {}",
            text(&response)
        );
        assert!(
            persisted(&acks).is_empty(),
            "case {case:?}: nothing is acknowledged persisted for a refused session, got {}",
            text(&response)
        );
        assert!(
            harness.stored_chunks(&arn).is_empty(),
            "case {case:?}: nothing is taken for a refused session"
        );
    }
}

/// RULE: a fault in the submitted MEDIA is reported in band — inside the same `200`, as
/// an `ERROR` acknowledgement naming the fault — and it ENDS the session at the Cluster
/// that carried it: the Clusters taken before it stay ingested, nothing after it is taken,
/// and the session reports no `PERSISTED` acknowledgement at all, because a producer reads
/// those only while its session is live.
///
/// The rows span the fault classes: a Cluster timecode that goes back, a frame on a track
/// the container never declared, a producer timestamp that names an instant which has not
/// happened, and three shapes of container that do not read as Matroska at all.
#[tokio::test]
async fn a_media_fault_is_reported_in_band() {
    /// One fault case.
    struct Case {
        name: &'static str,
        body: Vec<u8>,
        /// How far the session's producer start timestamp is from now, in milliseconds:
        /// negative for a producer that has been running, positive for one whose clock
        /// names a moment that has not happened.
        producer_start_offset_millis: i64,
        /// The `ErrorCode` the acknowledgement stream must carry.
        expected_code: &'static str,
        /// How many fragments survive in the store, which the fault did not take back.
        surviving: usize,
    }

    let cases: Vec<Case> = vec![
        Case {
            name: "a cluster timecode that goes back",
            body: Container::of(vec![
                cluster(0, 2, 30, 0x41),
                cluster(2000, 2, 30, 0x42),
                cluster(1000, 2, 30, 0x43),
                cluster(3000, 2, 30, 0x44),
            ])
            .bytes(),
            producer_start_offset_millis: -600_000,
            expected_code: "FRAGMENT_TIMECODE_LESSER_THAN_PREVIOUS",
            surviving: 2,
        },
        Case {
            name: "a frame on a track the container did not declare",
            body: Container::of(vec![
                cluster(0, 2, 30, 0x46),
                cluster_on_track(1000, TRACK_NUMBER + 1, 2, 30, 0x47),
            ])
            .bytes(),
            producer_start_offset_millis: -600_000,
            expected_code: "TRACK_NUMBER_MISMATCH",
            surviving: 1,
        },
        Case {
            name: "a producer timestamp that has not happened yet",
            body: Container::of(vec![cluster(0, 2, 30, 0x48)]).bytes(),
            // Well beyond any clock difference between a producer and this service: the
            // allowance exists for skew, not for a producer naming a day it has not
            // reached.
            producer_start_offset_millis: 400 * 24 * 3_600_000,
            expected_code: "INVALID_PRODUCER_TIMESTAMP",
            surviving: 0,
        },
        Case {
            name: "bytes that are not Matroska at all",
            body: b"this is not a matroska document".to_vec(),
            producer_start_offset_millis: -600_000,
            expected_code: "INVALID_MKV_DATA",
            surviving: 0,
        },
        Case {
            name: "an empty body",
            body: Vec::new(),
            producer_start_offset_millis: -600_000,
            expected_code: "INVALID_MKV_DATA",
            surviving: 0,
        },
        Case {
            name: "a truncated container",
            body: {
                let whole = Container::of(vec![cluster(0, 3, 30, 0x45)]).bytes();
                whole[..whole.len() - 4].to_vec()
            },
            producer_start_offset_millis: -600_000,
            expected_code: "INVALID_MKV_DATA",
            surviving: 0,
        },
    ];

    for (index, case) in cases.into_iter().enumerate() {
        let Case {
            name: case,
            body,
            producer_start_offset_millis,
            expected_code,
            surviving,
        } = case;
        let harness = Harness::new();
        let name = format!("ingest-probe-fault-{index}");
        let arn = harness.create_stream(&name).await;
        let producer_start = if producer_start_offset_millis >= 0 {
            SystemTime::now() + Duration::from_millis(producer_start_offset_millis.unsigned_abs())
        } else {
            SystemTime::now() - Duration::from_millis(producer_start_offset_millis.unsigned_abs())
        };
        let response = harness
            .put_media(
                &ingest_headers(
                    ("x-amzn-stream-name", name.clone()),
                    "RELATIVE",
                    producer_start,
                ),
                body,
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "case {case:?}: a media fault is reported in band, so the status stays \
             200, got {} ({}) {}",
            status(&response),
            error_code(&response),
            text(&response)
        );
        let acks = acknowledgements(case, &response);
        let errors: Vec<String> = acks
            .iter()
            .filter(|ack| event_type(ack) == "ERROR")
            .map(|ack| {
                ack.get("ErrorCode")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            errors,
            vec![expected_code.to_string()],
            "case {case:?}: expected one ERROR acknowledgement of {expected_code}, got {}",
            text(&response)
        );
        assert!(
            acks.iter().any(|ack| event_type(ack) == "ERROR"
                && ack.get("ErrorId").and_then(serde_json::Value::as_u64).is_some()),
            "case {case:?}: an ERROR acknowledgement states its ErrorId, got {}",
            text(&response)
        );
        // The session ended at the fault, so it reports no PERSISTED acknowledgement —
        // whatever it had already taken.
        let persisted_numbers = persisted(&acks);
        assert!(
            persisted_numbers.is_empty(),
            "case {case:?}: a session a fault ended reports no PERSISTED acknowledgement, \
             got {persisted_numbers:?}"
        );
        let received: Vec<&serde_json::Value> = acks
            .iter()
            .filter(|ack| event_type(ack) == "RECEIVED")
            .collect();
        assert_eq!(
            received.len(),
            surviving,
            "case {case:?}: expected {surviving} fragment(s) acknowledged as received, \
             got {}",
            text(&response)
        );
        let listed = harness
            .list_fragments(&serde_json::json!({ "StreamName": name }))
            .await;
        assert_eq!(
            listed.len(),
            surviving,
            "case {case:?}: expected {surviving} archived fragment(s), got {listed:?}"
        );
        assert_eq!(
            harness.stored_chunks(&arn).len(),
            surviving,
            "case {case:?}: expected {surviving} stored chunk(s)"
        );
    }
}

/// RULE: a producer's whole round trip works through the address the control plane
/// announces — `GetDataEndpoint(APIName=PUT_MEDIA)` answers a bare origin, and the ingest
/// plane answers the `POST /putMedia` the producer builds from it.
///
/// The rows differ in the address the caller reached the emulator on: a loopback address,
/// which is what a client on this machine resolves, and the service's published regional
/// host, which is what a client that resolves AWS's own hostnames addresses. Which
/// authority the announced value carries is the control plane's own rule (its
/// `stream_data_endpoint` suite drives the listener it opens); what this suite pins is
/// that the producer's call BUILT from that value — the endpoint with the API name
/// appended — is served by this plane and lands fragments.
#[tokio::test]
async fn a_producer_reaches_the_ingest_plane_at_the_announced_write_endpoint() {
    // The address a client on the machine this emulator runs on reaches it at.
    const LOCAL_ENDPOINT: &str = "127.0.0.1:9297";
    let producer_start = SystemTime::UNIX_EPOCH + Duration::from_millis(1_761_000_500_000);

    for (index, authority) in [LOCAL_ENDPOINT, ENDPOINT_HOST].into_iter().enumerate() {
        let harness = Harness::new();
        let name = format!("ingest-probe-endpoint-{index}");
        let created = harness
            .call_at(
                authority,
                "/createStream",
                &serde_json::json!({ "StreamName": name, "DataRetentionInHours": 1 }),
            )
            .await;
        assert_eq!(
            status(&created),
            200,
            "case {authority:?}: expected HTTP 200 from CreateStream, got {} {}",
            status(&created),
            text(&created)
        );
        harness.await_active(&name).await;

        let announced = harness
            .call_at(
                authority,
                "/getDataEndpoint",
                &serde_json::json!({ "StreamName": name, "APIName": "PUT_MEDIA" }),
            )
            .await;
        assert_eq!(
            status(&announced),
            200,
            "case {authority:?}: expected HTTP 200 from GetDataEndpoint, got {} {}",
            status(&announced),
            text(&announced)
        );
        let endpoint = json(&announced)
            .get("DataEndpoint")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let announced_scheme = format!("{}://", control_plane::state::DATA_PLANE_SCHEME);
        assert!(
            endpoint
                .strip_prefix(&announced_scheme)
                .is_some_and(|origin| !origin.is_empty() && !origin.contains('/')),
            "case {authority:?}: expected a bare {announced_scheme} origin with no path, \
             got {endpoint:?}"
        );

        // The producer appends the API name to what it was handed, exactly as the
        // documentation says it must, and sends its container there.
        let response = harness
            .put_media_at(
                &format!("{endpoint}/putMedia"),
                &ingest_headers(
                    ("x-amzn-stream-name", name.clone()),
                    "RELATIVE",
                    producer_start,
                ),
                Container::of(vec![cluster(0, 2, 30, 0x61), cluster(1000, 2, 30, 0x62)]).bytes(),
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "case {authority:?}: the announced write endpoint must serve POST /putMedia, \
             got {} ({}) {}",
            status(&response),
            error_code(&response),
            text(&response)
        );
        let numbers = persisted(&acknowledgements(authority, &response));
        assert_eq!(
            numbers.len(),
            2,
            "case {authority:?}: expected one PERSISTED acknowledgement per Cluster, got \
             {numbers:?}"
        );
        let listed = harness
            .list_fragments(&serde_json::json!({ "StreamName": name }))
            .await;
        assert_eq!(
            listed.len(),
            2,
            "case {authority:?}: the archive must hold both ingested fragments, got {listed:?}"
        );
    }
}

/// The fragment numbers the media log holds for the stream `arn` names, in order.
fn stored_fragment_numbers(harness: &Harness, arn: &str) -> Vec<String> {
    let state = media::state::service_state(&*harness.engine).expect("the media store");
    let store = state.lock();
    store
        .log_by_arn(arn)
        .map(|log| {
            log.fragments
                .iter()
                .map(|fragment| fragment.fragment_number.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: what one stream HOLDS is bounded — this emulator keeps a stream's media in
/// memory, so once the log is at its ceiling the OLDEST fragments are dropped to make
/// room for the newest — and a cursor a reader was already handed still names the
/// fragment it was minted for after that drop.
#[tokio::test]
async fn one_streams_log_is_bounded_and_a_cursor_survives_the_drop() {
    let ceiling = media::state::MAX_STORED_FRAGMENTS_PER_STREAM;
    let harness = Harness::new();
    let name = "ingest-probe-ceiling";
    let arn = harness.create_stream(name).await;
    let producer_start = SystemTime::UNIX_EPOCH + Duration::from_millis(1_761_000_600_000);
    let headers = ingest_headers(
        ("x-amzn-stream-name", name.to_string()),
        "RELATIVE",
        producer_start,
    );

    // Fill the log exactly to its ceiling: one tiny frame per Cluster, so the count
    // ceiling is what binds and the test stays small.
    let filling = Container::of(
        (0..ceiling)
            .map(|at| cluster(at as u64 * 10, 1, 0, (at % 251) as u8))
            .collect(),
    );
    let filled = harness.put_media(&headers, filling.bytes()).await;
    assert_eq!(
        status(&filled),
        200,
        "case \"fill the log\": expected HTTP 200, got {} {}",
        status(&filled),
        text(&filled)
    );
    let held = stored_fragment_numbers(&harness, &arn);
    assert_eq!(
        held.len(),
        ceiling,
        "case \"fill the log\": expected the log at its ceiling of {ceiling}, got {}",
        held.len()
    );

    // A cursor into the middle of that log, minted by a read BEFORE anything is dropped.
    let read = harness
        .get_media_earliest(&serde_json::json!({ "StreamName": name }))
        .await;
    let framing = tags_of(read.body());
    assert_eq!(
        framing.len(),
        ceiling,
        "case \"read the full log\": expected one framing element per chunk, got {}",
        framing.len()
    );
    let cursor_at = 9usize;
    let token = framing[cursor_at]
        .iter()
        .find(|(tag, _)| tag == "AWS_KINESISVIDEO_CONTINUATION_TOKEN")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    // The token resumes immediately AFTER the chunk that carried it.
    let resumes_at = held[cursor_at + 1].clone();

    // Five more fragments: the five oldest go.
    let overflowing = Container::of(
        (0..5)
            .map(|at| cluster(100_000 + at as u64 * 10, 1, 0, 0x7E))
            .collect(),
    );
    let overflowed = harness.put_media(&headers, overflowing.bytes()).await;
    assert_eq!(
        status(&overflowed),
        200,
        "case \"overflow the log\": expected HTTP 200, got {} {}",
        status(&overflowed),
        text(&overflowed)
    );
    let after = stored_fragment_numbers(&harness, &arn);
    assert_eq!(
        after.len(),
        ceiling,
        "case \"overflow the log\": the log must stay at its ceiling, got {}",
        after.len()
    );
    assert_eq!(
        after.first().map(String::as_str),
        held.get(5).map(String::as_str),
        "case \"overflow the log\": expected the five oldest fragments dropped, got the \
         earliest held {:?}",
        after.first()
    );
    let newest = (ceiling + 5).to_string();
    assert_eq!(
        after.last().map(String::as_str),
        Some(newest.as_str()),
        "case \"overflow the log\": expected the newest ingested fragment {newest} last, \
         got {:?}",
        after.last()
    );

    // The cursor still names the fragment it was minted for, whose older neighbours have
    // since gone.
    let resumed = harness
        .call(
            "/getMedia",
            &serde_json::json!({
                "StreamName": name,
                "StartSelector": {
                    "StartSelectorType": "CONTINUATION_TOKEN",
                    "ContinuationToken": token,
                },
            }),
        )
        .await;
    assert_eq!(
        status(&resumed),
        200,
        "case \"resume after the drop\": expected HTTP 200, got {} ({}) {}",
        status(&resumed),
        error_code(&resumed),
        text(&resumed)
    );
    let delivered = tags_of(resumed.body())
        .first()
        .and_then(|tags| {
            tags.iter()
                .find(|(tag, _)| tag == "AWS_KINESISVIDEO_FRAGMENT_NUMBER")
                .map(|(_, value)| value.clone())
        })
        .unwrap_or_default();
    assert_eq!(
        delivered, resumes_at,
        "case \"resume after the drop\": the cursor must still resume at fragment \
         {resumes_at}, got {delivered:?}"
    );

    // A cursor whose OWN fragment went with the drop is no longer held: it resolves to
    // nothing rather than silently resuming somewhere else.
    let stale = framing[0]
        .iter()
        .find(|(tag, _)| tag == "AWS_KINESISVIDEO_CONTINUATION_TOKEN")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    let refused = harness
        .call(
            "/getMedia",
            &serde_json::json!({
                "StreamName": name,
                "StartSelector": {
                    "StartSelectorType": "CONTINUATION_TOKEN",
                    "ContinuationToken": stale,
                },
            }),
        )
        .await;
    assert_eq!(
        status(&refused),
        400,
        "case \"a cursor behind the drop\": expected HTTP 400, got {} ({}) {}",
        status(&refused),
        error_code(&refused),
        text(&refused)
    );
    assert_eq!(
        error_code(&refused),
        "InvalidArgumentException",
        "case \"a cursor behind the drop\": expected InvalidArgumentException, got {:?}",
        error_code(&refused)
    );
}

/// RULE: one request is read only as far as its own ceiling — the Clusters past it are
/// not read, so nothing is acknowledged or persisted for them and a producer that reads
/// no acknowledgement for a fragment sends it again, exactly as it does when a session
/// ends mid-stream.
#[tokio::test]
async fn a_container_past_one_requests_ceiling_is_taken_only_as_far_as_the_ceiling() {
    let ceiling = media::state::MAX_STORED_FRAGMENTS_PER_STREAM
        .min(media::service::matroska::MAX_CLUSTERS_PER_REQUEST);
    let harness = Harness::new();
    let name = "ingest-probe-oversized";
    let arn = harness.create_stream(name).await;
    let producer_start = SystemTime::UNIX_EPOCH + Duration::from_millis(1_761_000_700_000);

    // Six Clusters more than one request may be read for.
    let submitted = ceiling + 6;
    let container = Container::of(
        (0..submitted)
            .map(|at| cluster(at as u64 * 10, 1, 0, (at % 251) as u8))
            .collect(),
    );
    let response = harness
        .put_media(
            &ingest_headers(
                ("x-amzn-stream-name", name.to_string()),
                "RELATIVE",
                producer_start,
            ),
            container.bytes(),
        )
        .await;
    assert_eq!(
        status(&response),
        200,
        "case \"over the request ceiling\": expected HTTP 200, got {} ({})",
        status(&response),
        error_code(&response)
    );

    let acks = acknowledgements("over the request ceiling", &response);
    let taken = persisted(&acks);
    assert_eq!(
        taken.len(),
        ceiling,
        "case \"over the request ceiling\": expected {ceiling} acknowledged fragment(s), \
         got {}",
        taken.len()
    );
    // What was acknowledged is the PREFIX of what was submitted, in order, and the
    // Clusters past the ceiling are acknowledged as nothing at all — not as an error, and
    // not as taken.
    let timecodes: Vec<u64> = taken.iter().map(|(_, at)| *at).collect();
    let expected: Vec<u64> = (0..ceiling).map(|at| at as u64 * 10).collect();
    assert_eq!(
        timecodes, expected,
        "case \"over the request ceiling\": expected the first {ceiling} Cluster \
         timecodes acknowledged, got {timecodes:?}"
    );
    assert!(
        acks.iter().all(|ack| event_type(ack) != "ERROR"),
        "case \"over the request ceiling\": a container read only as far as the ceiling \
         is not a fault, got {}",
        text(&response)
    );
    assert_eq!(
        harness.stored_chunks(&arn).len(),
        ceiling,
        "case \"over the request ceiling\": expected {ceiling} stored chunk(s)"
    );
    let listed = harness
        .list_fragments(&serde_json::json!({ "StreamName": name }))
        .await;
    // The archive holds every fragment that was taken; one `ListFragments` PAGE reports
    // at most the modelled maximum (`ListFragmentsMaxResults`, range 1..1000), so what
    // this one call answers is that page of them.
    let page = ceiling.min(1000);
    assert_eq!(
        listed.len(),
        page,
        "case \"over the request ceiling\": expected a page of {page} archived \
         fragment(s), got {}",
        listed.len()
    );
}

/// RULE: the ingest plane is claimed by THIS service, under the routing key its four
/// peers share, and only for the request a producer actually sends to it.
#[test]
fn the_ingest_route_is_claimed_only_for_the_producers_request() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = media::EmulatedService::new(engine);

    // (case, method, path, signing name, claimed)
    let rows: Vec<(&str, &str, &str, Option<&str>, bool)> = vec![
        (
            "the producer's own request",
            "POST",
            "/putMedia",
            Some("kinesisvideo"),
            true,
        ),
        (
            "the ingest route with a query string",
            "POST",
            "/putMedia?x=1",
            Some("kinesisvideo"),
            true,
        ),
        (
            "the modelled read route",
            "POST",
            "/getMedia",
            Some("kinesisvideo"),
            true,
        ),
        (
            "the ingest route under another service's signing name",
            "POST",
            "/putMedia",
            Some("kinesis"),
            false,
        ),
        (
            "the ingest route read, not written",
            "GET",
            "/putMedia",
            Some("kinesisvideo"),
            false,
        ),
        (
            "the ingest route in the wrong case",
            "POST",
            "/putmedia",
            Some("kinesisvideo"),
            false,
        ),
        (
            "a peer's route",
            "POST",
            "/listFragments",
            Some("kinesisvideo"),
            false,
        ),
        (
            "the ingest route with no routing signal",
            "POST",
            "/putMedia",
            None,
            false,
        ),
    ];

    for (case, method, path, signing_name, claimed) in rows {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://emulator.local{path}"))
            .header("host", "emulator.local");
        if let Some(signing_name) = signing_name {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261031/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            );
        }
        let (parts, ()) = builder.body(()).expect("a well-formed request").into_parts();
        let actual = service.can_handle(&parts);
        assert_eq!(
            actual, claimed,
            "case {case:?} ({method} {path}): expected claimed={claimed}, got {actual}"
        );
    }
}

/// RULE: a stored chunk is a Matroska document of its own — the submitted header in
/// front of one Cluster — so a reader handed one chunk can read it without the rest of
/// the session.
#[tokio::test]
async fn each_stored_chunk_is_a_matroska_document_of_its_own() {
    let harness = Harness::new();
    let name = "ingest-probe-chunks";
    let arn = harness.create_stream(name).await;
    let producer_start = SystemTime::UNIX_EPOCH + Duration::from_millis(1_761_000_400_000);
    let container = Container::of(vec![
        cluster(0, 2, 20, 0x51),
        cluster(1000, 1, 0, 0x52),
    ]);

    let response = harness
        .put_media(
            &ingest_headers(
                ("x-amzn-stream-name", name.to_string()),
                "RELATIVE",
                producer_start,
            ),
            container.bytes(),
        )
        .await;
    assert_eq!(
        status(&response),
        200,
        "case \"chunks\": expected HTTP 200, got {} {}",
        status(&response),
        text(&response)
    );

    for (index, chunk) in harness.stored_chunks(&arn).iter().enumerate() {
        // Every chunk begins with the EBML header the producer submitted, read back
        // through the inverse reader this test target carries.
        let (id, _, _) = read_ebml_element(chunk).unwrap_or_else(|| {
            panic!("case \"chunk {index}\": the chunk does not begin with an EBML element")
        });
        assert_eq!(
            id, ID_EBML,
            "case \"chunk {index}\": expected the EBML header first, got the element {id:?}"
        );
    }

    // And what GetMedia frames each chunk with is that chunk's own metadata: the
    // continuation token differs per chunk, so the second chunk resumes after the first.
    let read = harness
        .get_media_earliest(&serde_json::json!({ "StreamName": name }))
        .await;
    let payload = read.body();
    let tokens: Vec<String> = tags_of(payload)
        .iter()
        .map(|tags| {
            tags.iter()
                .find(|(name, _)| name == "AWS_KINESISVIDEO_CONTINUATION_TOKEN")
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        tokens.len(),
        2,
        "case \"chunks\": expected one framing element per delivered chunk, got {tokens:?}"
    );
    assert_ne!(
        tokens[0], tokens[1],
        "case \"chunks\": each chunk carries the token that resumes after it, got {tokens:?}"
    );
}

/// The `AWS_KINESISVIDEO_*` tag sets a `GetMedia` payload carries, one per delivered
/// chunk, found by scanning for the framing element the response adds after each chunk.
fn tags_of(payload: &[u8]) -> Vec<Vec<(String, String)>> {
    let mut found = Vec::new();
    for offset in 0..payload.len() {
        if payload[offset..].starts_with(ID_TAGS) {
            let tags = read_kvs_tags(&payload[offset..]);
            if !tags.is_empty() {
                found.push(tags);
            }
        }
    }
    found
}
