// SPDX-License-Identifier: CC0-1.0

//! The clip a consumer downloads is an MP4 FILE of the media a producer ingested.
//!
//! The rule under test: **`GetClip` answers an MP4 container muxed from the archived
//! Matroska fragments of the selected range** — "Downloads an MP4 file (clip) containing
//! the archived, on-demand media from the specified video stream over the specified time
//! range" — so the frames the fragments carry on their H.264 track become the samples of
//! the clip's one video track, with that track's `CodecPrivate` (the AVC decoder
//! configuration record) as the sample description. The preconditions of that mux are what
//! the codec-private-data errors report: a track that declares none is HTTP 400
//! `MissingCodecPrivateDataException`.
//!
//! Three crates have to agree for that to hold — the control plane creates the stream and
//! announces the planes, Kinesis Video Media ingests the container on `/putMedia`, and
//! Kinesis Video Archived Media clips the fragments it persisted — so the suite lives here.
//! The container is written in the test, from the element ids the Matroska specification
//! fixes and the EBML variable-size integers of RFC 8794 section 4.4, so the bytes the mux
//! reads are not bytes the code under test produced.
//!
//! The rows differ along what the rule generalises over: whether the ingested track
//! declares codec private data at all, and how many fragments of how many frames the
//! session submitted.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call here is signed for.
const REGION: &str = "us-west-2";

/// The control-plane address the calls that create the stream and discover its planes
/// arrive on.
const CONTROL_AUTHORITY: &str = "127.0.0.1:9297";

/// How long the suite keeps looking for a condition the clock owes — far longer than any
/// span this emulator uses, so a loaded host still passes.
const DEADLINE: Duration = Duration::from_secs(30);

/// How long it waits between looks.
const LOOK_AGAIN: Duration = Duration::from_millis(10);

// ---------------------------------------------------------------------------
// The Matroska container a producer submits.
// ---------------------------------------------------------------------------

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
/// Matroska `CodecPrivate`.
const ID_CODEC_PRIVATE: &[u8] = &[0x63, 0xA2];
/// Matroska `Video` (a `TrackEntry`'s video settings).
const ID_VIDEO: &[u8] = &[0xE0];
/// Matroska `PixelWidth`.
const ID_PIXEL_WIDTH: &[u8] = &[0xB0];
/// Matroska `PixelHeight`.
const ID_PIXEL_HEIGHT: &[u8] = &[0xBA];
/// Matroska `Cluster`.
const ID_CLUSTER: &[u8] = &[0x1F, 0x43, 0xB6, 0x75];
/// Matroska `Timestamp` (a Cluster's own timecode).
const ID_TIMESTAMP: &[u8] = &[0xE7];
/// Matroska `SimpleBlock` (one frame).
const ID_SIMPLE_BLOCK: &[u8] = &[0xA3];

/// Matroska `TrackType` 1: a video track.
const TRACK_TYPE_VIDEO: u64 = 1;

/// The `SimpleBlock` flag that says the frame is a keyframe.
const KEYFRAME_FLAG: u8 = 0x80;

/// The default Matroska timecode scale: one millisecond per tick, in nanoseconds.
const MILLISECOND_SCALE_NANOS: u64 = 1_000_000;

/// The Matroska codec id of H.264, the one the Kinesis Video Streams producers write.
const AVC_CODEC_ID: &str = "V_MPEG4/ISO/AVC";

/// The track number the submitted container declares and writes its frames on.
const TRACK_NUMBER: u64 = 1;

/// The geometry the submitted container declares for its track.
const PIXEL_WIDTH: u64 = 64;
/// Its height.
const PIXEL_HEIGHT: u64 = 48;

/// Append the EBML variable-size integer (RFC 8794 section 4.4) encoding `size`, in the
/// shortest width that holds it.
fn push_ebml_size(out: &mut Vec<u8>, size: u64) {
    for width in 1..=8u32 {
        let bits = 7 * width;
        // The all-ones data bits of a width are the "unknown size" encoding, so a size
        // that would need them goes in the next width up.
        let largest = (1u64 << bits) - 2;
        if size <= largest {
            let value = (1u64 << bits) | size;
            let bytes = value.to_be_bytes();
            out.extend_from_slice(&bytes[8 - width as usize..]);
            return;
        }
    }
    panic!("case an element body of {size} bytes: larger than the largest EBML size");
}

/// Append one EBML element: its id (itself already a variable-size integer), its size,
/// then `body`.
fn push_ebml_element(out: &mut Vec<u8>, id: &[u8], body: &[u8]) {
    out.extend_from_slice(id);
    push_ebml_size(out, body.len() as u64);
    out.extend_from_slice(body);
}

/// `value` as a Matroska unsigned-integer element body: big-endian, shortest form.
fn uint_body(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let first = bytes.iter().position(|byte| *byte != 0).unwrap_or(7);
    bytes[first..].to_vec()
}

/// The AVC decoder configuration record (ISO/IEC 14496-15) the submitted track declares:
/// the configuration version, the profile, its compatibility and the level, the four-byte
/// length framing of the units inside a frame, then one sequence and one picture parameter
/// set, each preceded by its own 16-bit big-endian length.
fn avc_record() -> Vec<u8> {
    let sequence_set: &[u8] = &[0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9, 0x40];
    let picture_set: &[u8] = &[0x68, 0xEE, 0x3C, 0xB0];
    let mut record = vec![
        1,
        sequence_set[1],
        sequence_set[2],
        sequence_set[3],
        // `lengthSizeMinusOne` of 3 — four-byte lengths — with the reserved bits set.
        0xFF,
        // One sequence parameter set, with the reserved bits set.
        0xE1,
    ];
    record.extend_from_slice(&(sequence_set.len() as u16).to_be_bytes());
    record.extend_from_slice(sequence_set);
    record.push(1);
    record.extend_from_slice(&(picture_set.len() as u16).to_be_bytes());
    record.extend_from_slice(picture_set);
    record
}

/// One frame of the submitted media: its units in the length-prefixed framing the track's
/// record states, carrying `marker` so one frame's bytes cannot be mistaken for another's.
fn frame(marker: u8) -> Vec<u8> {
    // One unit: a four-byte big-endian length, the NAL header of a coded slice of an
    // instantaneous decoding refresh picture (type 5), then the slice's own bytes.
    let unit: Vec<u8> = vec![0x65, marker, marker ^ 0x5A, 0xA7, marker.wrapping_add(7), 0x1D];
    let mut framed = (unit.len() as u32).to_be_bytes().to_vec();
    framed.extend_from_slice(&unit);
    framed
}

/// The container a producer submits: one Cluster per fragment, carrying `frames_each`
/// frames spaced a frame apart, with the track declaring `codec_private` or nothing.
fn container(clusters: usize, frames_each: usize, codec_private: Option<Vec<u8>>) -> Vec<u8> {
    document(clusters, frames_each, codec_private, submitted_frame)
}

/// One ARCHIVED fragment as this family holds it: a Matroska document of its own —
/// one Cluster, one frame that `marker` makes distinct — declaring the H.264 track
/// and the decoder configuration record of that track.
///
/// Kinesis Video Streams archives every fragment as such a document, and the reads
/// of the archive look inside it: a streaming session determines the type of its
/// media "from the codec IDs of the tracks in the first fragment" and its
/// initialization fragment from that track's codec private data. So a suite that
/// seeds an archive directly seeds this, the container an ingest would have
/// written, rather than a bare marker no read can classify.
pub(crate) fn archived_fragment(marker: u8) -> Vec<u8> {
    document(1, 1, Some(avc_record()), |_, _| frame(marker))
}

/// The Matroska document of `clusters` Clusters of `frames_each` frames, each frame
/// the one `frame_of` gives for its place, with the track declaring `codec_private`
/// or nothing.
fn document(
    clusters: usize,
    frames_each: usize,
    codec_private: Option<Vec<u8>>,
    frame_of: impl Fn(usize, usize) -> Vec<u8>,
) -> Vec<u8> {
    let mut document = Vec::new();
    let mut header = Vec::new();
    push_ebml_element(&mut header, ID_DOC_TYPE, b"matroska");
    push_ebml_element(&mut document, ID_EBML, &header);

    let mut segment = Vec::new();
    let mut info = Vec::new();
    push_ebml_element(
        &mut info,
        ID_TIMESTAMP_SCALE,
        &uint_body(MILLISECOND_SCALE_NANOS),
    );
    push_ebml_element(&mut segment, ID_INFO, &info);

    let mut entry = Vec::new();
    push_ebml_element(&mut entry, ID_TRACK_NUMBER, &uint_body(TRACK_NUMBER));
    push_ebml_element(&mut entry, ID_TRACK_TYPE, &uint_body(TRACK_TYPE_VIDEO));
    push_ebml_element(&mut entry, ID_CODEC_ID, AVC_CODEC_ID.as_bytes());
    if let Some(private) = &codec_private {
        push_ebml_element(&mut entry, ID_CODEC_PRIVATE, private);
    }
    let mut video = Vec::new();
    push_ebml_element(&mut video, ID_PIXEL_WIDTH, &uint_body(PIXEL_WIDTH));
    push_ebml_element(&mut video, ID_PIXEL_HEIGHT, &uint_body(PIXEL_HEIGHT));
    push_ebml_element(&mut entry, ID_VIDEO, &video);
    let mut tracks = Vec::new();
    push_ebml_element(&mut tracks, ID_TRACK_ENTRY, &entry);
    push_ebml_element(&mut segment, ID_TRACKS, &tracks);

    for cluster in 0..clusters {
        let mut body = Vec::new();
        push_ebml_element(
            &mut body,
            ID_TIMESTAMP,
            &uint_body(cluster as u64 * 1_000),
        );
        for position in 0..frames_each {
            // A `SimpleBlock` body is the track number as a variable-size integer, the
            // frame's signed 16-bit offset from the Cluster timecode, one flags byte, then
            // the frame itself.
            let mut block = vec![0x80 | TRACK_NUMBER as u8];
            block.extend_from_slice(&(position as i16 * 33).to_be_bytes());
            block.push(KEYFRAME_FLAG);
            block.extend_from_slice(&frame_of(cluster, position));
            push_ebml_element(&mut body, ID_SIMPLE_BLOCK, &block);
        }
        push_ebml_element(&mut segment, ID_CLUSTER, &body);
    }
    push_ebml_element(&mut document, ID_SEGMENT, &segment);
    document
}

/// The frame the `position`-th block of the `cluster`-th Cluster carries.
fn submitted_frame(cluster: usize, position: usize) -> Vec<u8> {
    frame(u8::try_from(cluster * 8 + position + 1).expect("a small frame count"))
}

// ---------------------------------------------------------------------------
// The harness.
// ---------------------------------------------------------------------------

/// The emulator with the three services this suite exercises, mounted on ONE engine
/// exactly as the router mounts them, with request authentication off so each call carries
/// a credential scope (which names the Region and the service) without a computed
/// signature.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_kinesisvideomedia_awsacuityinletservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// The SigV4 credential scope every call carries: its service field is the routing key the
/// Kinesis Video services share.
fn credential_scope() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kinesisvideo/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// What one call answered.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The `content-type` header of the response, or `""` when it carries none.
    content_type: String,
    /// The response body, as served.
    bytes: Vec<u8>,
}

impl Answer {
    /// The body as JSON, or `Null` for a body that is not.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes).unwrap_or(Value::Null)
    }

    /// The error code the envelope names, or `""` for a body that names none.
    fn code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The `Message` the envelope carries, or `""` for a body that carries none.
    fn message(&self) -> String {
        self.json()
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// POST one restJson1 call to `authority`.
async fn call(router: &MultiServiceRouter, authority: &str, path: &str, body: &Value) -> Answer {
    post(
        router,
        authority,
        path,
        &[("content-type", "application/json".to_string())],
        serde_json::to_vec(body).expect("the request body serializes"),
    )
    .await
}

/// POST `body` to `authority` with `headers`, through the router.
async fn post(
    router: &MultiServiceRouter,
    authority: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Vec<u8>,
) -> Answer {
    let mut request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{authority}{path}"))
        .header("host", authority)
        .header("authorization", credential_scope());
    for (name, value) in headers {
        request = request.header(*name, value.clone());
    }
    let response = router
        .dispatch(request.body(Bytes::from(body)).expect("a well-formed request"))
        .await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        content_type,
        bytes: response.into_body().to_vec(),
    }
}

/// The authority the control plane announces for `api_name` on `stream`.
async fn data_authority(router: &MultiServiceRouter, stream: &str, api_name: &str) -> String {
    let answer = call(
        router,
        CONTROL_AUTHORITY,
        "/getDataEndpoint",
        &json!({ "StreamName": stream, "APIName": api_name }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {api_name:?}: GetDataEndpoint expected HTTP 200, got {answer:?}"
    );
    let endpoint = answer
        .json()
        .get("DataEndpoint")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {api_name:?}: no DataEndpoint in {answer:?}"))
        .to_string();
    endpoint
        .split_once("://")
        .map(|(_, authority)| authority.to_string())
        .unwrap_or_else(|| panic!("case {api_name:?}: expected an origin, got {endpoint:?}"))
}

/// Create `stream` with a retention of one hour and wait until the control plane reports it
/// `ACTIVE`, which is what a client does before reading a stream it just created
/// ("CreateStream is an asynchronous operation").
///
/// Waits for the CONDITION, not for a span: it looks again until the status holds, under a
/// deadline far longer than the transition needs.
async fn given_active_stream(router: &MultiServiceRouter, stream: &str) {
    let created = call(
        router,
        CONTROL_AUTHORITY,
        "/createStream",
        &json!({ "StreamName": stream, "MediaType": "video/h264", "DataRetentionInHours": 1 }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create {stream:?}: expected HTTP 200, got {created:?}"
    );
    let deadline = Instant::now() + DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        let described = call(
            router,
            CONTROL_AUTHORITY,
            "/describeStream",
            &json!({ "StreamName": stream }),
        )
        .await;
        last = described.json()["StreamInfo"]["Status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if last == "ACTIVE" {
            return;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!("case create {stream:?}: expected Status ACTIVE before the deadline, got {last:?}");
}

/// Ingest `body` for `stream` through the announced `PUT_MEDIA` endpoint, as a producer
/// does.
async fn put_media(router: &MultiServiceRouter, stream: &str, body: Vec<u8>) -> Answer {
    let authority = data_authority(router, stream, "PUT_MEDIA").await;
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
        / 1000.0;
    post(
        router,
        &authority,
        "/putMedia",
        &[
            ("x-amzn-stream-name", stream.to_string()),
            ("x-amzn-fragment-timecode-type", "RELATIVE".to_string()),
            ("x-amzn-producer-start-timestamp", format!("{seconds:.3}")),
            ("x-amz-content-sha256", "UNSIGNED-PAYLOAD".to_string()),
        ],
        body,
    )
    .await
}

/// Wait until `ListFragments` reports `count` fragments of `stream`, as a consumer does
/// before reading media a producer has just sent.
///
/// Waits for the CONDITION, not for a span.
async fn await_fragments(router: &MultiServiceRouter, stream: &str, count: usize) {
    let authority = data_authority(router, stream, "LIST_FRAGMENTS").await;
    let body = json!({
        "StreamName": stream,
        "FragmentSelector": {
            "FragmentSelectorType": "SERVER_TIMESTAMP",
            "TimestampRange": { "StartTimestamp": 0, "EndTimestamp": 4_000_000_000_i64 },
        },
    });
    let deadline = Instant::now() + DEADLINE;
    let mut last = Answer {
        status: 0,
        content_type: String::new(),
        bytes: Vec::new(),
    };
    while Instant::now() < deadline {
        last = call(router, &authority, "/listFragments", &body).await;
        let listed = last.json()["Fragments"].as_array().map_or(0, Vec::len);
        if listed >= count {
            return;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!("case {stream:?}: expected {count} archived fragment(s), got {last:?}");
}

/// Clip the archive of `stream` through the announced `GET_CLIP` endpoint.
///
/// The window runs from an hour ago to the instant the clip is asked for, at the
/// millisecond precision the archive stamps a fragment at, which spans every
/// fragment this suite has ingested by then (a producer sends them as the test
/// runs, and the clip is asked for only once the archive holds them) while
/// meeting both clauses `ClipTimestampRange$EndTimestamp` documents: the end
/// "must be within 24 hours of the specified StartTimestamp", and, because this
/// selector reads the timestamps the SERVICE stamps, it "must be in the past".
async fn get_clip(router: &MultiServiceRouter, stream: &str) -> Answer {
    let authority = data_authority(router, stream, "GET_CLIP").await;
    let now_millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let hour_millis = 3_600_000_i64;
    call(
        router,
        &authority,
        "/getClip",
        &json!({
            "StreamName": stream,
            "ClipFragmentSelector": {
                "FragmentSelectorType": "SERVER_TIMESTAMP",
                "TimestampRange": {
                    "StartTimestamp": (now_millis - hour_millis) as f64 / 1000.0,
                    "EndTimestamp": now_millis as f64 / 1000.0,
                },
            },
        }),
    )
    .await
}

/// Whether `clip` carries `needle` somewhere in it — how a client checks that the media it
/// ingested is the media the clip is of.
fn carries(clip: &[u8], needle: &[u8]) -> bool {
    clip.windows(needle.len()).any(|window| window == needle)
}

// ---------------------------------------------------------------------------
// The rule.
// ---------------------------------------------------------------------------

/// THE RULE: the clip of a stream a producer ingested Matroska into is an MP4 file whose
/// media data carries the access units of those fragments, served as `video/mp4`.
///
/// The rows differ along how many fragments the session submitted and how many frames each
/// of them carries.
#[tokio::test(flavor = "multi_thread")]
async fn a_clip_of_ingested_matroska_is_an_mp4_of_its_frames() {
    let router = emulator();

    // (case, Clusters submitted, frames per Cluster)
    let rows: [(&str, usize, usize); 3] = [
        ("one fragment of one frame", 1, 1),
        ("two fragments of one frame", 2, 1),
        ("two fragments of three frames", 2, 3),
    ];

    for (case, clusters, frames_each) in rows {
        let stream = format!("clip-probe-{clusters}-{frames_each}");
        given_active_stream(&router, &stream).await;
        let ingested = put_media(
            &router,
            &stream,
            container(clusters, frames_each, Some(avc_record())),
        )
        .await;
        assert_eq!(
            ingested.status, 200,
            "case {case:?}: PutMedia expected HTTP 200, got {ingested:?}"
        );
        await_fragments(&router, &stream, clusters).await;

        let clip = get_clip(&router, &stream).await;
        assert_eq!(
            clip.status, 200,
            "case {case:?}: GetClip expected HTTP 200, got {clip:?}"
        );
        assert_eq!(
            clip.content_type, "video/mp4",
            "case {case:?}: GetClip downloads an MP4 clip, got {:?}",
            clip.content_type
        );
        // The first top-level box of an MP4 file is its File Type box: a four-byte
        // big-endian size, then the box type `ftyp` (ISO/IEC 14496-12). What a client
        // probes before handing the payload to a demuxer.
        assert_eq!(
            clip.bytes.get(4..8),
            Some(b"ftyp".as_slice()),
            "case {case:?}: expected an MP4 container, got {:?}",
            &clip.bytes[..clip.bytes.len().min(12)]
        );
        for cluster in 0..clusters {
            for position in 0..frames_each {
                let submitted = submitted_frame(cluster, position);
                assert!(
                    carries(&clip.bytes, &submitted),
                    "case {case:?}: the clip must carry the access unit ingested as frame \
                     {position} of fragment {cluster}, got {} bytes carrying none of it",
                    clip.bytes.len()
                );
            }
        }
    }
}

/// THE OTHER HALF: the mux needs the track's codec private data, so a stream whose track
/// declared none is refused HTTP 400 `MissingCodecPrivateDataException` naming that track —
/// while the sibling stream whose track DID declare it is clipped.
#[tokio::test(flavor = "multi_thread")]
async fn a_track_that_declares_no_codec_private_data_cannot_be_clipped() {
    let router = emulator();

    // One row: what the ingested track declares as its codec private data, and the answer
    // a clip of that stream earns.
    struct Row {
        case: &'static str,
        stream: &'static str,
        codec_private: Option<Vec<u8>>,
        status: u16,
        code: &'static str,
        message: String,
    }

    let rows = [
        Row {
            case: "a track that declares its decoder configuration record",
            stream: "clip-codec-private-declared",
            codec_private: Some(avc_record()),
            status: 200,
            code: "",
            message: String::new(),
        },
        Row {
            case: "a track that declares none",
            stream: "clip-codec-private-absent",
            codec_private: None,
            status: 400,
            code: "MissingCodecPrivateDataException",
            message: format!("Missing codec private data in fragment for track {TRACK_NUMBER}."),
        },
    ];

    for Row {
        case,
        stream,
        codec_private,
        status,
        code,
        message,
    } in rows
    {
        given_active_stream(&router, stream).await;
        let ingested = put_media(&router, stream, container(1, 2, codec_private)).await;
        assert_eq!(
            ingested.status, 200,
            "case {case:?}: PutMedia expected HTTP 200, got {ingested:?}"
        );
        await_fragments(&router, stream, 1).await;

        let clip = get_clip(&router, stream).await;
        assert_eq!(
            clip.status, status,
            "case {case:?}: expected HTTP {status}, got {clip:?}"
        );
        assert_eq!(
            clip.code(),
            code,
            "case {case:?}: expected {code:?}, got {clip:?}"
        );
        if status == 200 {
            assert_eq!(
                clip.bytes.get(4..8),
                Some(b"ftyp".as_slice()),
                "case {case:?}: expected an MP4 container, got {:?}",
                &clip.bytes[..clip.bytes.len().min(12)]
            );
        } else {
            assert_eq!(
                clip.message(),
                message,
                "case {case:?}: expected {message:?}, got {clip:?}"
            );
        }
    }
}
