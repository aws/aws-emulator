// SPDX-License-Identifier: CC0-1.0

//! The archived-media read loop, end to end over HTTP.
//!
//! Every test drives the real `MultiServiceRouter` with this one service
//! registered, so a request goes through routing, restJson1 deserialization,
//! constraint validation, the handler, and serialization — the same path a
//! socket-served request takes.
//!
//! The archive is SEEDED through the service's own state API (this service has
//! no create operation: streams and fragments are ingested through the Kinesis
//! Video Streams control/data plane), and every assertion below is about what
//! the handlers compute FROM that archive. A seeded stream is REGISTERED in the
//! Kinesis Video Streams registry under the same identity ([`register_stream`]),
//! because that registry is where a stream exists: a read resolves it there and
//! then serves the archive that record's own ARN is held under, exactly as an
//! ingest writes it.

use bytes::Bytes;
use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as control_plane;
use service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader as archived_media;
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use archived_media::state::{
    epoch_millis, from_epoch_millis, service_state, ArchivedStream, StoredFragment,
};

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";
/// The endpoint every request in this suite addresses: this emulator, on
/// loopback. A session URL must come back here, never to a real AWS host.
pub(crate) const EMULATOR_ENDPOINT: &str = "127.0.0.1:9297";
/// The seeded stream with an archive and 24 hours of retention.
pub(crate) const STREAM: &str = "demo-stream";
/// A seeded stream with `DataRetentionInHours` of 0.
pub(crate) const NO_RETENTION_STREAM: &str = "no-retention-stream";
/// A seeded stream whose fragments were ingested just now.
pub(crate) const LIVE_STREAM: &str = "live-stream";
/// The base of the seeded range: an instant well in the past, so nothing in
/// this stream's archive is within the LIVE 30-second window.
pub(crate) const T0_SECS: i64 = 1_761_000_000;
/// How many fragments `STREAM` holds.
const SEEDED_FRAGMENTS: u64 = 5;
/// How many fragments `LIVE_STREAM` holds.
pub(crate) const LIVE_FRAGMENTS: u64 = 7;
/// A realistic Kinesis Video fragment number: a long, increasing decimal.
const FRAGMENT_BASE: u64 = 9_134_385_233_318_143_200;

/// The fragment number of the `index`-th seeded fragment.
pub(crate) fn fragment_number(index: u64) -> String {
    (FRAGMENT_BASE + index).to_string()
}

/// The media bytes of the `index`-th seeded fragment: one archived Matroska
/// fragment carrying [`FRAMES_PER_FRAGMENT`] of the fixture's coded pictures,
/// distinct per fragment, so a payload can only come out right if the right
/// fragments were picked in the right order.
pub(crate) fn media(index: u64) -> Vec<u8> {
    MkvFragment::of(first_unit(index), FRAMES_PER_FRAGMENT).bytes()
}

/// The access units the `index`-th seeded fragment carries, in order — what a
/// clip of that fragment has to put in its track.
fn media_units(index: u64) -> Vec<Vec<u8>> {
    access_units(first_unit(index), FRAMES_PER_FRAGMENT)
}

/// Where the `index`-th seeded fragment's pictures start in the fixture's coded
/// video.
fn first_unit(index: u64) -> usize {
    usize::try_from(index).expect("small index") * FRAMES_PER_FRAGMENT
}

// ---------------------------------------------------------------------------
// The archived fragment: a Matroska document carrying coded H.264.
// ---------------------------------------------------------------------------
//
// Kinesis Video Streams archives each fragment as a Matroska document of its own
// (the submitted header in front of one Cluster), so the seeded media is one. The
// element ids below are the Matroska specification's and the sizes are EBML
// variable-size integers (RFC 8794 section 4.4), written here in the test rather
// than by the code under test. The frames are really encoded H.264 and the
// track's `CodecPrivate` is really that encoding's decoder configuration record,
// so the clip's samples and its sample description are a codec's own rather than
// bytes shaped to look like one.

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
/// Matroska `Video` (the video settings of a `TrackEntry`).
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

/// The Matroska codec id of H.264, as the codec registry assigns it and the
/// Kinesis Video Streams producers write it.
pub(crate) const AVC_CODEC_ID: &str = "V_MPEG4/ISO/AVC";

/// How many pictures the fixture's coded video carries. More than one fragment
/// and more than one window of the clip's fragment ceiling need, so two
/// selections of the archive carry different pictures.
const CODED_PICTURES: usize = 16;

/// The width the fixture's pictures are coded at, in pixels.
const CODED_WIDTH: u32 = 64;
/// Their height, in pixels. Even sides, as the codec's chroma sampling needs.
const CODED_HEIGHT: u32 = 48;

/// How many coded pictures one seeded fragment carries.
const FRAMES_PER_FRAGMENT: usize = 2;

/// How far apart the frames of one fragment are shown, in the container's own
/// timecode ticks.
const FRAME_SPACING_TICKS: i16 = 33;

/// The fixture's coded video: the parameter sets of the track that carries it,
/// and one access unit per coded picture in the length-prefixed framing an MP4
/// track carries a sample in — the framing a Kinesis Video Streams producer
/// writes its frames in, so one fragment frame is one of these.
struct CodedVideo {
    seq_param_set: Vec<u8>,
    pic_param_set: Vec<u8>,
    access_units: Vec<Vec<u8>>,
}

/// One fixture picture: a bright bar whose place follows `index`, so two coded
/// pictures differ and a clip that carried the wrong one cannot pass.
fn fixture_picture(index: usize) -> Vec<u8> {
    let mut rgb = vec![16u8; (CODED_WIDTH * CODED_HEIGHT * 3) as usize];
    let column = (index as u32 * 4) % CODED_WIDTH;
    for row in 0..CODED_HEIGHT {
        for offset in 0..4u32 {
            let x = (column + offset) % CODED_WIDTH;
            let at = ((row * CODED_WIDTH + x) * 3) as usize;
            if let Some(pixel) = rgb.get_mut(at..at + 3) {
                pixel.copy_from_slice(&[235, 235, 235]);
            }
        }
    }
    rgb
}

/// The fixture's coded video, encoded ONCE for the whole suite.
fn coded_video() -> &'static CodedVideo {
    static CODED: std::sync::OnceLock<CodedVideo> = std::sync::OnceLock::new();
    CODED.get_or_init(|| {
        let mut encoder = ::openh264::encoder::Encoder::with_api_config(
            ::openh264::OpenH264API::from_source(),
            ::openh264::encoder::EncoderConfig::new(),
        )
        .expect("the fixture's encoder starts");
        let mut stream: Vec<u8> = Vec::new();
        for index in 0..CODED_PICTURES {
            let rgb = fixture_picture(index);
            let yuv = ::openh264::formats::YUVBuffer::from_rgb8_source(
                ::openh264::formats::RgbSliceU8::new(
                    &rgb,
                    (CODED_WIDTH as usize, CODED_HEIGHT as usize),
                ),
            );
            stream.extend_from_slice(
                &encoder
                    .encode(&yuv)
                    .expect("the fixture's picture encodes")
                    .to_vec(),
            );
        }
        let units = ::service_common::h264_mp4::annex_b_units(&stream);
        let access_units: Vec<Vec<u8>> = ::service_common::h264_mp4::avcc_samples(&units)
            .into_iter()
            .map(|sample| sample.bytes)
            .collect();
        assert_eq!(
            access_units.len(),
            CODED_PICTURES,
            "case the fixture's coded video: expected one access unit per picture, got {}",
            access_units.len()
        );
        CodedVideo {
            seq_param_set: ::service_common::h264_mp4::parameter_set(
                &units,
                ::service_common::h264_mp4::SEQUENCE_PARAMETER_SET,
            )
            .expect("the fixture carries a sequence parameter set"),
            pic_param_set: ::service_common::h264_mp4::parameter_set(
                &units,
                ::service_common::h264_mp4::PICTURE_PARAMETER_SET,
            )
            .expect("the fixture carries a picture parameter set"),
            access_units,
        }
    })
}

/// `count` access units of the fixture's coded video from `first`, wrapping round
/// the pictures it carries.
fn access_units(first: usize, count: usize) -> Vec<Vec<u8>> {
    let coded = coded_video();
    (0..count)
        .map(|offset| coded.access_units[(first + offset) % CODED_PICTURES].clone())
        .collect()
}

/// The AVC decoder configuration record (ISO/IEC 14496-15) a track declares as
/// its `CodecPrivate`: the configuration version, the profile, its compatibility
/// and the level the sequence parameter set states, the four-byte length framing
/// of the units inside a frame, then the sequence and the picture parameter set,
/// each preceded by its own 16-bit big-endian length.
pub(crate) fn avc_record(seq_param_set: &[u8], pic_param_set: &[u8]) -> Vec<u8> {
    let mut record = vec![
        1,
        seq_param_set.get(1).copied().unwrap_or_default(),
        seq_param_set.get(2).copied().unwrap_or_default(),
        seq_param_set.get(3).copied().unwrap_or_default(),
        // `lengthSizeMinusOne` of 3: four-byte lengths, with the reserved bits set.
        0xFF,
        // One sequence parameter set, with the reserved bits set.
        0xE1,
    ];
    record.extend_from_slice(&u16::try_from(seq_param_set.len()).unwrap_or(0).to_be_bytes());
    record.extend_from_slice(seq_param_set);
    record.push(1);
    record.extend_from_slice(&u16::try_from(pic_param_set.len()).unwrap_or(0).to_be_bytes());
    record.extend_from_slice(pic_param_set);
    record
}

/// The record of the fixture's own coded video — the one an archived fragment of
/// that video declares.
pub(crate) fn fixture_record() -> Vec<u8> {
    let coded = coded_video();
    avc_record(&coded.seq_param_set, &coded.pic_param_set)
}

/// One archived fragment, as a Matroska document.
#[derive(Debug, Clone)]
pub(crate) struct MkvFragment {
    /// The `TrackNumber` the track is declared with and every frame is written on.
    pub(crate) track: u64,
    /// The track's `CodecID`.
    pub(crate) codec_id: String,
    /// Its `CodecPrivate`, or `None` for a track that declares none.
    pub(crate) codec_private: Option<Vec<u8>>,
    /// The `PixelWidth`/`PixelHeight` the container declares for the track, or
    /// `None` for a container that declares neither.
    pub(crate) pixel_size: Option<(u64, u64)>,
    /// `Info/TimestampScale`: nanoseconds per timecode tick.
    pub(crate) scale_nanos: u64,
    /// The Cluster's own `Timestamp`, in those ticks.
    pub(crate) timecode: u64,
    /// How far apart its frames are shown, in those ticks.
    pub(crate) spacing_ticks: i16,
    /// The frames the Cluster carries, in order.
    pub(crate) frames: Vec<Vec<u8>>,
}

impl MkvFragment {
    /// A fragment of the fixture's coded video: `count` access units from
    /// `first`, on track 1, declaring that video's own record and leaving the
    /// geometry to the coded video itself.
    pub(crate) fn of(first: usize, count: usize) -> Self {
        Self {
            track: 1,
            codec_id: AVC_CODEC_ID.to_string(),
            codec_private: Some(fixture_record()),
            pixel_size: None,
            scale_nanos: MILLISECOND_SCALE_NANOS,
            timecode: 0,
            spacing_ticks: FRAME_SPACING_TICKS,
            frames: access_units(first, count),
        }
    }

    /// The document's bytes.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        let mut document = Vec::new();
        let mut header = Vec::new();
        push_ebml_element(&mut header, ID_DOC_TYPE, b"matroska");
        push_ebml_element(&mut document, ID_EBML, &header);

        let mut segment = Vec::new();
        let mut info = Vec::new();
        push_ebml_element(&mut info, ID_TIMESTAMP_SCALE, &uint_body(self.scale_nanos));
        push_ebml_element(&mut segment, ID_INFO, &info);

        let mut entry = Vec::new();
        push_ebml_element(&mut entry, ID_TRACK_NUMBER, &uint_body(self.track));
        push_ebml_element(&mut entry, ID_TRACK_TYPE, &uint_body(TRACK_TYPE_VIDEO));
        push_ebml_element(&mut entry, ID_CODEC_ID, self.codec_id.as_bytes());
        if let Some(private) = &self.codec_private {
            push_ebml_element(&mut entry, ID_CODEC_PRIVATE, private);
        }
        if let Some((width, height)) = self.pixel_size {
            let mut video = Vec::new();
            push_ebml_element(&mut video, ID_PIXEL_WIDTH, &uint_body(width));
            push_ebml_element(&mut video, ID_PIXEL_HEIGHT, &uint_body(height));
            push_ebml_element(&mut entry, ID_VIDEO, &video);
        }
        let mut tracks = Vec::new();
        push_ebml_element(&mut tracks, ID_TRACK_ENTRY, &entry);
        push_ebml_element(&mut segment, ID_TRACKS, &tracks);

        let mut cluster = Vec::new();
        push_ebml_element(&mut cluster, ID_TIMESTAMP, &uint_body(self.timecode));
        for (position, frame) in self.frames.iter().enumerate() {
            // A `SimpleBlock` body is the track number as a variable-size
            // integer, the frame's signed 16-bit offset from the Cluster
            // timecode, one flags byte, then the frame itself.
            let offset = self
                .spacing_ticks
                .saturating_mul(i16::try_from(position).expect("a small frame count"));
            let mut block = vec![0x80 | u8::try_from(self.track).expect("a small track number")];
            block.extend_from_slice(&offset.to_be_bytes());
            block.push(KEYFRAME_FLAG);
            block.extend_from_slice(frame);
            push_ebml_element(&mut cluster, ID_SIMPLE_BLOCK, &block);
        }
        push_ebml_element(&mut segment, ID_CLUSTER, &cluster);
        push_ebml_element(&mut document, ID_SEGMENT, &segment);
        document
    }
}

/// Append the EBML variable-size integer (RFC 8794 section 4.4) encoding `size`,
/// in the shortest width that holds it.
fn push_ebml_size(out: &mut Vec<u8>, size: u64) {
    for width in 1..=8u32 {
        let bits = 7 * width;
        // The all-ones data bits of a width are the "unknown size" encoding, so a
        // size that would need them goes in the next width up.
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

/// Append one EBML element: its id (itself already a variable-size integer), its
/// size, then `body`.
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

// ---------------------------------------------------------------------------
// The clip, read back as a client's demuxer reads it.
// ---------------------------------------------------------------------------

/// What the clip's video track carries, read through a maintained ISO base media
/// reader.
pub(crate) struct ClipTrack {
    /// The visible width the track states, in pixels.
    pub(crate) width: u16,
    /// Its visible height.
    pub(crate) height: u16,
    /// The sequence parameter set of its sample description.
    pub(crate) seq_param_set: Vec<u8>,
    /// Its picture parameter set.
    pub(crate) pic_param_set: Vec<u8>,
    /// Its samples, in decode order.
    pub(crate) samples: Vec<Vec<u8>>,
    /// Which of them a player may start decoding at.
    pub(crate) sync: Vec<bool>,
}

/// The clip's one video track, failing `case` when the bytes are not an MP4 file
/// carrying one.
pub(crate) fn clip_track(case: &str, clip: &[u8]) -> ClipTrack {
    let size = clip.len() as u64;
    let mut reader = ::mp4::Mp4Reader::read_header(std::io::Cursor::new(clip), size)
        .unwrap_or_else(|error| {
            panic!("case {case}: the clip must read as an MP4 file, got {error}")
        });
    let (id, width, height, seq_param_set, pic_param_set, count) = {
        let track = reader
            .tracks()
            .values()
            .find(|track| track.track_type().ok() == Some(::mp4::TrackType::Video))
            .unwrap_or_else(|| panic!("case {case}: the clip must carry a video track"));
        assert_eq!(
            track.media_type().ok(),
            Some(::mp4::MediaType::H264),
            "case {case}: the clip's video track must be H.264"
        );
        (
            track.track_id(),
            track.width(),
            track.height(),
            track
                .sequence_parameter_set()
                .unwrap_or_else(|error| {
                    panic!("case {case}: the track must state a sequence parameter set, got {error}")
                })
                .to_vec(),
            track
                .picture_parameter_set()
                .unwrap_or_else(|error| {
                    panic!("case {case}: the track must state a picture parameter set, got {error}")
                })
                .to_vec(),
            track.sample_count(),
        )
    };
    let mut samples = Vec::new();
    let mut sync = Vec::new();
    for sample_id in 1..=count {
        let sample = reader
            .read_sample(id, sample_id)
            .ok()
            .flatten()
            .unwrap_or_else(|| panic!("case {case}: sample {sample_id} must be readable"));
        samples.push(sample.bytes.to_vec());
        sync.push(sample.is_sync);
    }
    ClipTrack {
        width,
        height,
        seq_param_set,
        pic_param_set,
        samples,
        sync,
    }
}

/// The first bytes of an MP4 file are its File Type box: a four-byte big-endian
/// size, then the box type `ftyp` (ISO/IEC 14496-12). What a client probes before
/// handing a payload to a demuxer.
pub(crate) fn assert_is_mp4(case: &str, clip: &[u8]) {
    assert_eq!(
        clip.get(4..8),
        Some(b"ftyp".as_slice()),
        "case {case}: a clip must be an MP4 container, got {:?}",
        &clip[..clip.len().min(12)]
    );
}

/// The producer timestamp of the `index`-th seeded fragment.
fn producer_secs(index: u64) -> i64 {
    T0_SECS + 10 * i64::try_from(index).expect("small index")
}

/// The server timestamp of the `index`-th seeded fragment: an hour's worth of
/// offset from the producer timestamp, so selecting on one field cannot pass by
/// accident when the other was meant.
fn server_secs(index: u64) -> i64 {
    producer_secs(index) + 60
}

/// A seeded emulator: the router, the shared engine, and the seeded stream's ARN.
pub(crate) struct Harness {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
    pub(crate) arn: String,
}

impl Harness {
    /// POST `body` to `path` through the router, signed for `REGION` so the
    /// request routes by its SigV4 credential scope, and addressed at a
    /// loopback endpoint — the emulator's own, never a real AWS host.
    pub(crate) async fn post(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> http::Response<Bytes> {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{EMULATOR_ENDPOINT}{path}"))
            .header("host", EMULATOR_ENDPOINT)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kinesisvideo/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("a valid request");
        self.router.dispatch(request).await
    }

    /// Fetch `url` the way a MEDIA PLAYER does: an unsigned `GET` carrying no
    /// credentials at all, which is how a player follows the streaming-session URL
    /// a session call reported.
    ///
    /// The URL is sent as the service reported it, so what this exercises is the
    /// address the emulator published rather than a path a suite composed.
    pub(crate) async fn fetch(&self, url: &str) -> http::Response<Bytes> {
        self.fetch_with("GET", url).await
    }

    /// [`Self::fetch`] with the method the case names, for the one rule that is
    /// about which methods a playback address answers.
    pub(crate) async fn fetch_with(&self, method: &str, url: &str) -> http::Response<Bytes> {
        let request = http::Request::builder()
            .method(method)
            .uri(url)
            .body(Bytes::new())
            .expect("a valid request");
        self.router.dispatch(request).await
    }
}

/// The JSON body of a response.
pub(crate) fn body_json(response: &http::Response<Bytes>) -> serde_json::Value {
    serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
}

/// The `content-type` header of a response.
fn content_type(response: &http::Response<Bytes>) -> String {
    response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// Register `archived` in the Kinesis Video Streams registry, under the very
/// identity its archive is held with, the way `CreateStream` plus the ingest
/// plane leave the two stores.
///
/// A stream EXISTS in that registry, so an archived-media read resolves it there
/// and serves the archive the resolved record's own `StreamARN` is held under —
/// which is what an ingest writes, since the archive it creates carries the ARN
/// the registry holds. Seeding an archive alone would therefore leave a stream no
/// read resolves, so every seeded stream is registered here too, `ACTIVE` with
/// nothing the clock owes it.
pub(crate) fn register_stream(engine: &engine::InMemoryEngine, archived: &ArchivedStream) {
    let registry = control_plane::state::service_state(engine).expect("the control-plane store");
    let mut registry = registry.lock();
    let version = registry.mint_version();
    registry
        .stream_arn_index
        .insert(archived.arn.clone(), archived.name.clone());
    registry.streams.insert(
        archived.name.clone(),
        control_plane::state::Stream {
            stream_name: archived.name.clone(),
            stream_arn: archived.arn.clone(),
            device_name: None,
            media_type: Some(archived.media_type.clone()),
            kms_key_id: control_plane::state::default_kms_key_id(REGION),
            version,
            status: control_plane::state::STATUS_ACTIVE.to_string(),
            creation_time_millis: control_plane::state::now_millis(),
            data_retention_in_hours: i32::try_from(archived.data_retention_in_hours)
                .unwrap_or(i32::MAX),
            tags: BTreeMap::new(),
            default_storage_tier: None,
            activates_at: None,
            deletes_at: None,
            notification_configuration: None,
            image_generation_configuration: None,
            edge: None,
        },
    );
}

/// A router serving this service alone over an engine seeded with three
/// streams: an archive in the past, one with no retention, and one ingested
/// just now.
pub(crate) fn seeded() -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let arn = {
        let state = service_state(&*engine).expect("the service store");
        let mut state = state.lock();

        let created_at = from_epoch_millis(T0_SECS * 1000);
        for archived in [
            ArchivedStream::new(STREAM, REGION, created_at, 24),
            ArchivedStream::new(NO_RETENTION_STREAM, REGION, created_at, 0),
            ArchivedStream::new(LIVE_STREAM, REGION, created_at, 24),
        ] {
            register_stream(&engine, &archived);
            state.put_stream(archived);
        }

        // Appended out of ingestion order on purpose: the archive is kept in
        // ascending fragment-number order by the state API, not by the caller.
        for index in [2, 0, 4, 1, 3] {
            assert!(
                state.append_fragment(
                    STREAM,
                    StoredFragment::new(
                        fragment_number(index),
                        from_epoch_millis(producer_secs(index) * 1000),
                        from_epoch_millis(server_secs(index) * 1000),
                        2000,
                        media(index),
                    ),
                ),
                "case seed fragment {index}: the seeded stream must exist"
            );
        }

        let now = SystemTime::now();
        for index in 0..LIVE_FRAGMENTS {
            let ingested = now - Duration::from_secs(LIVE_FRAGMENTS - index);
            assert!(
                state.append_fragment(
                    LIVE_STREAM,
                    StoredFragment::new(
                        fragment_number(index),
                        ingested,
                        ingested,
                        1000,
                        media(index),
                    ),
                ),
                "case seed live fragment {index}: the live stream must exist"
            );
        }

        state
            .streams
            .get(STREAM)
            .expect("the seeded stream")
            .arn
            .clone()
    };

    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(archived_media::EmulatedService::new(Arc::clone(&engine)));
    Harness {
        router,
        engine,
        arn,
    }
}

/// `base` with every member of `extra` added.
pub(crate) fn with_members(
    base: &serde_json::Value,
    extra: &serde_json::Value,
) -> serde_json::Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    for (key, value) in extra.as_object().expect("an object of members") {
        object.insert(key.clone(), value.clone());
    }
    serde_json::Value::Object(object)
}

/// Every operation of the read loop, with a request body that succeeds for the
/// seeded stream once the stream-naming members are added.
pub(crate) fn read_loop_operations() -> Vec<(&'static str, serde_json::Value)> {
    vec![
        (
            "/listFragments",
            serde_json::json!({
                "FragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
        ),
        (
            "/getMediaForFragmentList",
            serde_json::json!({ "Fragments": [fragment_number(0)] }),
        ),
        (
            "/getClip",
            serde_json::json!({
                "ClipFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
        ),
        (
            "/getHLSStreamingSessionURL",
            serde_json::json!({
                "PlaybackMode": "ON_DEMAND",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
        ),
        (
            "/getDASHStreamingSessionURL",
            serde_json::json!({
                "PlaybackMode": "ON_DEMAND",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
        ),
    ]
}

/// RULE: every archived-media operation resolves the stream the same way —
/// exactly one of `StreamName` and `StreamARN` must be present (neither and
/// both are `InvalidArgumentException`, 400), and a name or ARN that answers to
/// no stream is `ResourceNotFoundException` (404).
#[tokio::test]
async fn every_operation_resolves_the_stream_by_the_same_rule() {
    let harness = seeded();
    let cases: Vec<(&str, serde_json::Value, u16, &str)> = vec![
        (
            "StreamName only",
            serde_json::json!({ "StreamName": STREAM }),
            200,
            "",
        ),
        (
            "StreamARN only",
            serde_json::json!({ "StreamARN": harness.arn.clone() }),
            200,
            "",
        ),
        (
            "both StreamName and StreamARN",
            serde_json::json!({ "StreamName": STREAM, "StreamARN": harness.arn.clone() }),
            400,
            "InvalidArgumentException",
        ),
        (
            "neither StreamName nor StreamARN",
            serde_json::json!({}),
            400,
            "InvalidArgumentException",
        ),
        (
            "unknown StreamName",
            serde_json::json!({ "StreamName": "no-such-stream" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown StreamARN",
            serde_json::json!({
                "StreamARN": "arn:aws:kinesisvideo:us-east-1:000000000000:stream/absent/1761000000"
            }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (path, extra) in read_loop_operations() {
        for (case, stream, status, error) in &cases {
            let body = with_members(stream, &extra);
            let response = harness.post(path, &body).await;
            assert_eq!(
                response.status().as_u16(),
                *status,
                "case {path} / {case}: expected HTTP {status}, got {} with body {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            if !error.is_empty() {
                let reported = body_json(&response);
                assert_eq!(
                    reported["__type"], *error,
                    "case {path} / {case}: expected error {error}, got {reported:?}"
                );
            }
        }
    }
}

/// RULE: `ListFragments` returns the archived fragments whose SELECTED
/// timestamp field (`PRODUCER_TIMESTAMP` or `SERVER_TIMESTAMP`) falls inside
/// the inclusive `TimestampRange` — "This value is inclusive. The EndTimestamp
/// is compared to the (starting) timestamp of the fragment."
#[tokio::test]
async fn list_fragments_filters_on_the_selected_timestamp_field() {
    let harness = seeded();
    // (case, selector type, start offset, end offset, the fragment indexes the
    // archive holds in that window)
    let cases: Vec<(&str, &str, i64, i64, Vec<u64>)> = vec![
        ("producer, whole archive", "PRODUCER_TIMESTAMP", 0, 1000, vec![0, 1, 2, 3, 4]),
        ("producer, first three", "PRODUCER_TIMESTAMP", 0, 25, vec![0, 1, 2]),
        ("producer, one instant", "PRODUCER_TIMESTAMP", 10, 10, vec![1]),
        ("producer, before the archive", "PRODUCER_TIMESTAMP", -100, -1, vec![]),
        ("producer, inclusive end", "PRODUCER_TIMESTAMP", 20, 30, vec![2, 3]),
        // The same windows read on the server timestamps, which are offset by
        // a minute: the SAME range selects a different set.
        ("server, producer window", "SERVER_TIMESTAMP", 0, 25, vec![]),
        ("server, first three", "SERVER_TIMESTAMP", 60, 85, vec![0, 1, 2]),
        ("server, last one", "SERVER_TIMESTAMP", 100, 1000, vec![4]),
        ("server, whole archive", "SERVER_TIMESTAMP", 0, 1000, vec![0, 1, 2, 3, 4]),
    ];

    for (case, selector_type, start, end, expected) in cases {
        let body = serde_json::json!({
            "StreamName": STREAM,
            "FragmentSelector": {
                "FragmentSelectorType": selector_type,
                "TimestampRange": {
                    "StartTimestamp": T0_SECS + start,
                    "EndTimestamp": T0_SECS + end,
                },
            },
        });
        let response = harness.post("/listFragments", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        let numbers: Vec<String> = reported["Fragments"]
            .as_array()
            .expect("Fragments is a list")
            .iter()
            .map(|fragment| fragment["FragmentNumber"].as_str().unwrap_or("").to_string())
            .collect();
        let wanted: Vec<String> = expected.iter().copied().map(fragment_number).collect();
        assert_eq!(
            numbers, wanted,
            "case {case}: expected fragments {wanted:?}, got {numbers:?}"
        );
        assert!(
            reported["NextToken"].is_null(),
            "case {case}: a page that returned everything must carry no NextToken, got {:?}",
            reported["NextToken"]
        );
    }
}

/// RULE: each returned `Fragment` reports that fragment's own persisted
/// identity, size, timestamps and duration.
#[tokio::test]
async fn list_fragments_reports_each_fragment_from_the_archive() {
    let harness = seeded();
    let body = serde_json::json!({
        "StreamName": STREAM,
        "FragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
        },
    });
    let response = harness.post("/listFragments", &body).await;
    assert_eq!(response.status().as_u16(), 200, "expected HTTP 200");
    let reported = body_json(&response);
    let fragments = reported["Fragments"].as_array().expect("Fragments is a list");

    for index in 0..SEEDED_FRAGMENTS {
        let reported = &fragments[usize::try_from(index).expect("small index")];
        let case = format!("fragment {index}");
        assert_eq!(
            reported["FragmentNumber"].as_str().unwrap_or_default(),
            fragment_number(index),
            "case {case}: wrong FragmentNumber, got {reported:?}"
        );
        assert_eq!(
            reported["FragmentSizeInBytes"].as_i64().unwrap_or_default(),
            i64::try_from(media(index).len()).expect("a small fragment"),
            "case {case}: FragmentSizeInBytes must be the stored media's length, got {reported:?}"
        );
        assert_eq!(
            reported["ProducerTimestamp"].as_f64().unwrap_or_default(),
            producer_secs(index) as f64,
            "case {case}: wrong ProducerTimestamp, got {reported:?}"
        );
        assert_eq!(
            reported["ServerTimestamp"].as_f64().unwrap_or_default(),
            server_secs(index) as f64,
            "case {case}: wrong ServerTimestamp, got {reported:?}"
        );
        assert_eq!(
            reported["FragmentLengthInMilliseconds"]
                .as_i64()
                .unwrap_or_default(),
            2000,
            "case {case}: wrong FragmentLengthInMilliseconds, got {reported:?}"
        );
    }
}

/// RULE: pagination is a partition — whatever the page size, replaying the
/// returned `NextToken` continues where the previous page stopped, no fragment
/// repeats or goes missing, and the final page carries no token. The token also
/// carries the selector, which is why the model makes `FragmentSelector`
/// optional on a continuation.
#[tokio::test]
async fn list_fragments_pages_partition_the_matching_fragments() {
    let harness = seeded();
    let all: Vec<String> = (0..SEEDED_FRAGMENTS).map(fragment_number).collect();

    for page_size in 1..=(SEEDED_FRAGMENTS + 1) {
        let case = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages: u64 = 0;

        loop {
            let body = match &next_token {
                // A continuation carries ONLY the token: no selector.
                Some(token) => serde_json::json!({
                    "StreamName": STREAM,
                    "MaxResults": page_size,
                    "NextToken": token,
                }),
                None => serde_json::json!({
                    "StreamName": STREAM,
                    "MaxResults": page_size,
                    "FragmentSelector": {
                        "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                        "TimestampRange": {
                            "StartTimestamp": T0_SECS,
                            "EndTimestamp": T0_SECS + 1000,
                        },
                    },
                }),
            };
            let response = harness.post("/listFragments", &body).await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case}: expected HTTP 200, got {} with body {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            let reported = body_json(&response);
            let page: Vec<String> = reported["Fragments"]
                .as_array()
                .expect("Fragments is a list")
                .iter()
                .map(|fragment| fragment["FragmentNumber"].as_str().unwrap_or("").to_string())
                .collect();
            assert!(
                page.len() <= usize::try_from(page_size).expect("small page size"),
                "case {case}: a page returned {} fragments, more than MaxResults",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(pages <= SEEDED_FRAGMENTS + 2, "case {case}: pagination did not terminate");

            match reported["NextToken"].as_str() {
                Some(token) => {
                    assert!(
                        token
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='),
                        "case {case}: NextToken {token} is outside the modelled token pattern"
                    );
                    next_token = Some(token.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            seen, all,
            "case {case}: paging must yield every matching fragment exactly once, in order"
        );
    }
}

/// RULE: a request that carries neither `NextToken` nor `FragmentSelector`, or
/// a range whose end precedes its start, or a token issued for another stream,
/// is `InvalidArgumentException` (400).
#[tokio::test]
async fn list_fragments_refuses_a_selector_it_cannot_page_on() {
    let harness = seeded();
    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "no selector and no token",
            serde_json::json!({ "StreamName": STREAM }),
        ),
        (
            "end before start",
            serde_json::json!({
                "StreamName": STREAM,
                "FragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS + 10,
                        "EndTimestamp": T0_SECS,
                    },
                },
            }),
        ),
        (
            "a token that is not one",
            serde_json::json!({ "StreamName": STREAM, "NextToken": "bm90YXRva2Vu" }),
        ),
    ];

    for (case, body) in cases {
        let response = harness.post("/listFragments", &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        assert_eq!(
            reported["__type"], "InvalidArgumentException",
            "case {case}: expected InvalidArgumentException, got {reported:?}"
        );
    }
}

/// The refusal a `ListFragments` continuation that names another listing than its token
/// was minted over is answered with.
const LIST_TOKEN_FOR_ANOTHER_QUERY: &str =
    "NextToken was issued for a different stream or timestamp range";

/// RULE: a `NextToken` names the listing it was minted for — the stream and the
/// selector's timestamp field and range — so a continuation is served against THAT
/// listing: the selector, "only required when the NextToken isn't passed in the API",
/// may be left out and is then the token's own, while one re-stated for another stream
/// or another range is refused HTTP 400 `InvalidArgumentException` rather than served a
/// page of another listing. `MaxResults`, which sizes a page rather than the listing,
/// may differ freely.
///
/// The rows differ along which part of the selector the continuation re-states
/// differently, and the served rows pin that the continuation resumes at the token's own
/// offset in the token's own listing.
#[tokio::test]
async fn list_fragments_refuses_a_continuation_that_names_another_listing() {
    let harness = seeded();
    let selector = |origin: &str, start: i64, end: i64| {
        serde_json::json!({
            "FragmentSelectorType": origin,
            "TimestampRange": { "StartTimestamp": start, "EndTimestamp": end },
        })
    };
    let minted = selector("PRODUCER_TIMESTAMP", T0_SECS, T0_SECS + 1000);
    let first = serde_json::json!({
        "StreamName": STREAM,
        "MaxResults": 1,
        "FragmentSelector": minted.clone(),
    });
    let response = harness.post("/listFragments", &first).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "the first page must be served, got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let token = body_json(&response)["NextToken"]
        .as_str()
        .expect("a one-fragment page of a five-fragment archive carries a NextToken")
        .to_string();
    // A continuation that re-states nothing optional: the stream and the token.
    let bare = serde_json::json!({
        "StreamName": STREAM,
        "MaxResults": 1,
        "NextToken": token,
    });
    // (case, what the continuation states on top of the bare one, the status expected)
    let rows: Vec<(&str, serde_json::Value, u16)> = vec![
        ("no selector at all", serde_json::json!({}), 200),
        (
            "the selector the token was minted with",
            serde_json::json!({ "FragmentSelector": minted.clone() }),
            200,
        ),
        (
            "a page size the first page did not use",
            serde_json::json!({ "MaxResults": 2 }),
            200,
        ),
        (
            "a range that ends elsewhere",
            serde_json::json!({
                "FragmentSelector": selector("PRODUCER_TIMESTAMP", T0_SECS, T0_SECS + 20),
            }),
            400,
        ),
        (
            "a range that starts elsewhere",
            serde_json::json!({
                "FragmentSelector": selector("PRODUCER_TIMESTAMP", T0_SECS + 1, T0_SECS + 1000),
            }),
            400,
        ),
        (
            "the other timestamp field",
            serde_json::json!({
                "FragmentSelector": selector("SERVER_TIMESTAMP", T0_SECS, T0_SECS + 1000),
            }),
            400,
        ),
        (
            "another stream",
            serde_json::json!({ "StreamName": LIVE_STREAM }),
            400,
        ),
    ];

    for (case, extra, status) in rows {
        let body = with_members(&bare, &extra);
        let response = harness.post("/listFragments", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        if status == 200 {
            // Served from the token's own listing, resumed at the token's own offset:
            // the second fragment of the archive the token was minted over.
            let resumed = reported["Fragments"][0]["FragmentNumber"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                resumed,
                fragment_number(1),
                "case {case}: expected the page to resume at the token's next fragment, got \
                 {:?}",
                reported["Fragments"]
            );
            continue;
        }
        assert_eq!(
            reported["__type"], "InvalidArgumentException",
            "case {case}: expected InvalidArgumentException, got {reported:?}"
        );
        let message = reported["Message"]
            .as_str()
            .or_else(|| reported["message"].as_str())
            .unwrap_or_default();
        assert_eq!(
            message, LIST_TOKEN_FOR_ANOTHER_QUERY,
            "case {case}: expected {LIST_TOKEN_FOR_ANOTHER_QUERY:?}, got {message:?}"
        );
    }
}

/// RULE: `GetMediaForFragmentList`'s payload is the stored media of exactly the
/// requested fragments, concatenated in the order the request lists them.
#[tokio::test]
async fn get_media_for_fragment_list_concatenates_exactly_what_was_asked_for() {
    let harness = seeded();
    // (case, the fragment indexes requested, in request order)
    let cases: Vec<(&str, Vec<u64>)> = vec![
        ("one fragment", vec![0]),
        ("two in archive order", vec![0, 1]),
        ("two in reverse order", vec![1, 0]),
        ("the same fragment twice", vec![2, 2]),
        ("a non-contiguous selection", vec![4, 1, 3]),
        ("the whole archive", vec![0, 1, 2, 3, 4]),
    ];

    for (case, indexes) in cases {
        let body = serde_json::json!({
            "StreamName": STREAM,
            "Fragments": indexes.iter().copied().map(fragment_number).collect::<Vec<_>>(),
        });
        let response = harness.post("/getMediaForFragmentList", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            content_type(&response),
            "video/webm",
            "case {case}: wrong ContentType header"
        );
        let expected: Vec<u8> = indexes.iter().copied().flat_map(media).collect();
        assert_eq!(
            response.body().as_ref(),
            expected.as_slice(),
            "case {case}: expected the concatenation of {indexes:?}, got {} bytes",
            response.body().len()
        );
    }
}

/// RULE: a fragment number the archive does not hold is
/// `InvalidArgumentException` (400) — never a short payload.
#[tokio::test]
async fn get_media_for_fragment_list_refuses_an_unarchived_fragment() {
    let harness = seeded();
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("only an absent fragment", vec![fragment_number(99)]),
        (
            "an absent fragment beside a present one",
            vec![fragment_number(0), fragment_number(99)],
        ),
        (
            "a fragment of another stream's numbering",
            vec!["1".to_string()],
        ),
    ];

    for (case, fragments) in cases {
        let body = serde_json::json!({ "StreamName": STREAM, "Fragments": fragments });
        let response = harness.post("/getMediaForFragmentList", &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        assert_eq!(
            reported["__type"], "InvalidArgumentException",
            "case {case}: expected InvalidArgumentException, got {reported:?}"
        );
    }
}

/// RULE: the bytes one response assembles are bounded, not just the number of
/// fragments a request may name — a list that names one large fragment
/// repeatedly is refused with the modelled limit error rather than
/// materialising N copies of it.
#[tokio::test]
async fn get_media_for_fragment_list_bounds_the_bytes_it_assembles() {
    use archived_media::state::MAX_PAYLOAD_BYTES;

    let harness = seeded();
    // One fragment just over half the ceiling, so TWO references to it exceed
    // it while one does not: the count is identical in both cases, only the
    // product differs.
    let large = fragment_number(SEEDED_FRAGMENTS + 1);
    {
        let state = service_state(&*harness.engine).expect("the service store");
        let mut state = state.lock();
        assert!(
            state.append_fragment(
                STREAM,
                StoredFragment::new(
                    large.clone(),
                    from_epoch_millis((T0_SECS + 500) * 1000),
                    from_epoch_millis((T0_SECS + 560) * 1000),
                    2000,
                    vec![b'z'; MAX_PAYLOAD_BYTES / 2 + 1],
                ),
            ),
            "case seed the large fragment: the seeded stream must exist"
        );
    }

    let cases: Vec<(&str, Vec<String>, u16)> = vec![
        ("one copy, under the ceiling", vec![large.clone()], 200),
        (
            "two copies, over the ceiling",
            vec![large.clone(), large.clone()],
            400,
        ),
    ];

    for (case, fragments, status) in cases {
        let body = serde_json::json!({ "StreamName": STREAM, "Fragments": fragments });
        let response = harness.post("/getMediaForFragmentList", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case}: expected HTTP {status}, got {}",
            response.status()
        );
        if status == 400 {
            let reported = body_json(&response);
            assert_eq!(
                reported["__type"], "ClientLimitExceededException",
                "case {case}: expected ClientLimitExceededException, got {reported:?}"
            );
        }
    }
}

/// RULE: `GetClip` answers `video/mp4` and an MP4 container muxed from exactly
/// the fragments the archive holds in the selected inclusive range — the frames
/// those fragments carry on their H.264 track become the samples of the clip's
/// one video track, in archive order, with that track's codec private data as the
/// sample description.
#[tokio::test]
async fn get_clip_muxes_the_fragments_the_archive_holds_in_range() {
    let harness = seeded();
    // (case, selector type, start offset, end offset, fragment indexes)
    let cases: Vec<(&str, &str, i64, i64, Vec<u64>)> = vec![
        ("producer, whole archive", "PRODUCER_TIMESTAMP", 0, 1000, vec![0, 1, 2, 3, 4]),
        ("producer, first two", "PRODUCER_TIMESTAMP", 0, 15, vec![0, 1]),
        ("producer, one fragment", "PRODUCER_TIMESTAMP", 30, 35, vec![3]),
        ("server, first three", "SERVER_TIMESTAMP", 60, 85, vec![0, 1, 2]),
        ("server, whole archive", "SERVER_TIMESTAMP", 0, 1000, vec![0, 1, 2, 3, 4]),
    ];

    for (case, selector_type, start, end, indexes) in cases {
        let body = serde_json::json!({
            "StreamName": STREAM,
            "ClipFragmentSelector": {
                "FragmentSelectorType": selector_type,
                "TimestampRange": {
                    "StartTimestamp": T0_SECS + start,
                    "EndTimestamp": T0_SECS + end,
                },
            },
        });
        let response = harness.post("/getClip", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            content_type(&response),
            "video/mp4",
            "case {case}: GetClip downloads an MP4 clip"
        );
        assert_is_mp4(case, response.body());
        let expected: Vec<Vec<u8>> = indexes.iter().copied().flat_map(media_units).collect();
        let track = clip_track(case, response.body());
        assert_eq!(
            track.samples,
            expected,
            "case {case}: expected the access units of the fragments {indexes:?} as the clip's \
             samples, got {} samples",
            track.samples.len()
        );
        assert_eq!(
            (track.width, track.height),
            (
                u16::try_from(CODED_WIDTH).expect("a short side"),
                u16::try_from(CODED_HEIGHT).expect("a short side")
            ),
            "case {case}: the clip's track must state the coded geometry, got {}x{}",
            track.width,
            track.height
        );
        assert_eq!(
            (track.seq_param_set, track.pic_param_set),
            (
                coded_video().seq_param_set.clone(),
                coded_video().pic_param_set.clone()
            ),
            "case {case}: the sample description must carry the track's own parameter sets"
        );
    }
}

/// RULE: a clip carries at most the documented number of fragments — "The
/// output will contain the first 100 MB or the first 200 fragments from the
/// specified start timestamp" — counted from the start of the range, so a range
/// holding more than that is TRUNCATED rather than refused or served whole.
#[tokio::test]
async fn get_clip_truncates_at_the_documented_fragment_ceiling() {
    /// The published ceiling this test pins, from the `GetClip$Payload` doc
    /// sentence quoted above.
    const MAX_CLIP_FRAGMENTS: usize = 200;
    /// A stream holding more fragments in range than one clip may carry.
    const DEEP_STREAM: &str = "deep-archive-stream";

    // One picture per fragment, taken from a different place in the fixture's
    // coded video, so the expected sample sequence pins WHICH fragments were
    // taken and in what order: a handler that returned the LAST 200 fragments of
    // the range, or any other 200, produces a different sequence of the same
    // length.
    let fragment = |index: usize| MkvFragment::of(index, 1);

    let harness = seeded();
    let held = MAX_CLIP_FRAGMENTS + 50;
    {
        let state = service_state(&*harness.engine).expect("the service store");
        let mut state = state.lock();
        let archived =
            ArchivedStream::new(DEEP_STREAM, REGION, from_epoch_millis(T0_SECS * 1000), 24);
        register_stream(&harness.engine, &archived);
        state.put_stream(archived);
        for index in 0..held {
            let at = from_epoch_millis((T0_SECS + index as i64) * 1000);
            assert!(
                state.append_fragment(
                    DEEP_STREAM,
                    StoredFragment::new(
                        fragment_number(index as u64),
                        at,
                        at,
                        1000,
                        fragment(index).bytes(),
                    ),
                ),
                "case seed fragment {index}: the deep stream must exist"
            );
        }
    }

    let body = serde_json::json!({
        "StreamName": DEEP_STREAM,
        "ClipFragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": {
                "StartTimestamp": T0_SECS,
                "EndTimestamp": T0_SECS + held as i64,
            },
        },
    });
    let response = harness.post("/getClip", &body).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case truncated clip: expected HTTP 200, got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    assert_eq!(
        content_type(&response),
        "video/mp4",
        "case truncated clip: a truncated clip is still an MP4 clip"
    );
    assert_is_mp4("truncated clip", response.body());
    let track = clip_track("truncated clip", response.body());
    let expected: Vec<Vec<u8>> = (0..MAX_CLIP_FRAGMENTS)
        .flat_map(|index| fragment(index).frames)
        .collect();
    assert_eq!(
        track.samples.len(),
        expected.len(),
        "case truncated clip: a clip must stop at the first {MAX_CLIP_FRAGMENTS} fragments of the \
         range, got {} samples for an archive of {held} fragments in range",
        track.samples.len()
    );
    assert_eq!(
        track.samples, expected,
        "case truncated clip: a clip must carry the frames of the FIRST {MAX_CLIP_FRAGMENTS} \
         fragments of the range, in order"
    );

    // The same range read through ListFragments still reports every fragment:
    // the ceiling is GetClip's own output limit, not a limit on the archive.
    let listed = harness
        .post(
            "/listFragments",
            &serde_json::json!({
                "StreamName": DEEP_STREAM,
                "MaxResults": 1000,
                "FragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + held as i64,
                    },
                },
            }),
        )
        .await;
    assert_eq!(
        listed.status().as_u16(),
        200,
        "case the archive behind the clip: expected HTTP 200, got {}",
        listed.status()
    );
    assert_eq!(
        body_json(&listed)["Fragments"]
            .as_array()
            .map_or(0, Vec::len),
        held,
        "case the archive behind the clip: ListFragments must still report every fragment in range"
    );
}

/// RULE: a stream that retains no data is `NoDataRetentionException` (400) —
/// "Data retention must be greater than 0" — and a range the archive holds no
/// fragment in is `ResourceNotFoundException` (404).
#[tokio::test]
async fn get_clip_refuses_a_stream_or_range_it_cannot_clip() {
    let harness = seeded();
    let cases: Vec<(&str, &str, i64, i64, u16, &str)> = vec![
        (
            "a stream with no retention",
            NO_RETENTION_STREAM,
            0,
            1000,
            400,
            "NoDataRetentionException",
        ),
        (
            "a range before the archive",
            STREAM,
            -1000,
            -1,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a range after the archive",
            STREAM,
            5000,
            6000,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a range between two fragments",
            STREAM,
            1,
            9,
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, stream, start, end, status, error) in cases {
        let body = serde_json::json!({
            "StreamName": stream,
            "ClipFragmentSelector": {
                "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                "TimestampRange": {
                    "StartTimestamp": T0_SECS + start,
                    "EndTimestamp": T0_SECS + end,
                },
            },
        });
        let response = harness.post("/getClip", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        assert_eq!(
            reported["__type"], error,
            "case {case}: expected {error}, got {reported:?}"
        );
    }
}

/// RULE: `GetHLSStreamingSessionURL` mints a session per call and RECORDS it —
/// its parameters, its expiry and the playlist snapshot it serves — so a second
/// call answers a different URL and both sessions exist in state.
#[tokio::test]
async fn get_hls_streaming_session_url_records_a_distinct_session_per_call() {
    let harness = seeded();
    let body = serde_json::json!({
        "StreamName": STREAM,
        "PlaybackMode": "ON_DEMAND",
        "Expires": 600,
        "HLSFragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
        },
    });

    let mut urls: Vec<String> = Vec::new();
    for call in 1..=2 {
        let response = harness.post("/getHLSStreamingSessionURL", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case call {call}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        let url = reported["HLSStreamingSessionURL"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            url.starts_with(&format!("https://{EMULATOR_ENDPOINT}/hls/v1/")),
            "case call {call}: the URL must address the endpoint the caller reached this \
             emulator on, got {url:?}"
        );
        urls.push(url);
    }

    assert_ne!(
        urls[0], urls[1],
        "case two calls: each call must mint its own session, got {urls:?}"
    );

    let state = service_state(&*harness.engine).expect("the service store");
    let state = state.lock();
    assert_eq!(
        state.sessions.len(),
        2,
        "case two calls: both sessions must be recorded, got {:?}",
        state.sessions.keys().collect::<Vec<_>>()
    );
    for (token, session) in &state.sessions {
        assert!(
            urls.iter().any(|url| url.ends_with(token)),
            "case session {token}: no returned URL carries this session's token"
        );
        assert_eq!(
            session.stream_name, STREAM,
            "case session {token}: wrong stream, got {session:?}"
        );
        assert_eq!(
            session.expires_in_secs, 600,
            "case session {token}: Expires must be honoured, got {session:?}"
        );
        assert_eq!(
            epoch_millis(session.expires_at()) - epoch_millis(session.created_at),
            600 * 1000,
            "case session {token}: the expiry must be created_at + Expires, got {session:?}"
        );
        assert_eq!(
            session.fragment_numbers,
            (0..SEEDED_FRAGMENTS).map(fragment_number).collect::<Vec<_>>(),
            "case session {token}: the playlist snapshot must be the fragments in range"
        );
    }
}

/// One row of the `GetHLSStreamingSessionURL` defaults table: the case name,
/// the selector type, the requested `MaxMediaPlaylistFragmentResults`, the
/// `DiscontinuityMode` the defaults imply, and the playlist snapshot expected.
type HlsDefaultsRow = (&'static str, &'static str, Option<u64>, &'static str, Vec<u64>);

/// RULE: the session store is bounded at the site that grows it — a session
/// whose `Expires` window the clock has already closed is dropped when the next
/// session is recorded, and live sessions never exceed `MAX_STREAMING_SESSIONS`.
#[tokio::test]
async fn minting_a_session_bounds_the_session_store() {
    use archived_media::state::{
        SessionProtocol, SessionWindow, StreamingSession, MAX_STREAMING_SESSIONS,
    };

    let harness = seeded();
    let body = serde_json::json!({
        "StreamName": STREAM,
        "PlaybackMode": "ON_DEMAND",
        "HLSFragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
        },
    });

    // A session recorded long enough ago that its own Expires window has
    // closed, beside one that is still live.
    let now = SystemTime::now();
    let session_at = |token: &str, created_at: SystemTime, expires_in_secs: u32| StreamingSession {
        token: token.to_string(),
        stream_name: STREAM.to_string(),
        protocol: SessionProtocol::Hls,
        playback_mode: "ON_DEMAND".to_string(),
        origin: archived_media::state::TimestampOrigin::Producer,
        container_format: "FRAGMENTED_MP4".to_string(),
        discontinuity_mode: Some("NEVER".to_string()),
        display_fragment_timestamp: "NEVER".to_string(),
        display_fragment_number: None,
        fragment_numbers: Vec::new(),
        window: SessionWindow::Range {
            start_millis: T0_SECS * 1000,
            end_millis: (T0_SECS + 1000) * 1000,
        },
        max_fragments: 1000,
        created_at,
        expires_in_secs,
    };
    {
        let state = service_state(&*harness.engine).expect("the service store");
        let mut state = state.lock();
        state.record_session(session_at("expired", now - Duration::from_secs(3600), 300));
        state.record_session(session_at("live", now, 43200));
    }

    let response = harness.post("/getHLSStreamingSessionURL", &body).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "expected HTTP 200, got {}",
        response.status()
    );

    {
        let state = service_state(&*harness.engine).expect("the service store");
        let state = state.lock();
        assert!(
            !state.sessions.contains_key("expired"),
            "case expired session: a session past its Expires window must be dropped, held {:?}",
            state.sessions.keys().collect::<Vec<_>>()
        );
        assert!(
            state.sessions.contains_key("live"),
            "case live session: a session inside its Expires window must be kept, held {:?}",
            state.sessions.keys().collect::<Vec<_>>()
        );
    }

    // And live sessions alone cannot grow the store without end: recording
    // more than the ceiling keeps the ceiling and drops the oldest first.
    {
        let state = service_state(&*harness.engine).expect("the service store");
        let mut state = state.lock();
        for ordinal in 0..(MAX_STREAMING_SESSIONS + 16) {
            let created_at = now + Duration::from_millis(ordinal as u64);
            state.record_session(session_at(&format!("live-{ordinal}"), created_at, 43200));
        }
        assert!(
            state.sessions.len() <= MAX_STREAMING_SESSIONS,
            "case over the ceiling: the store holds {} sessions, over the {MAX_STREAMING_SESSIONS} \
             ceiling",
            state.sessions.len()
        );
        assert!(
            !state.sessions.contains_key("live-0"),
            "case over the ceiling: the oldest session must be the one evicted"
        );
        assert!(
            state.sessions.contains_key(&format!("live-{}", MAX_STREAMING_SESSIONS + 15)),
            "case over the ceiling: the newest session must be kept"
        );
    }
}

/// RULE: the documented parameter defaults are what an unspecified member
/// means — `Expires` 300, `ContainerFormat` FRAGMENTED_MP4,
/// `DisplayFragmentTimestamp` NEVER, and `DiscontinuityMode` ALWAYS for
/// SERVER_TIMESTAMP / NEVER for PRODUCER_TIMESTAMP — and
/// `MaxMediaPlaylistFragmentResults` caps the playlist snapshot.
#[tokio::test]
async fn get_hls_streaming_session_url_applies_the_documented_defaults() {
    // (case, selector type, max results, expected discontinuity mode, snapshot)
    let cases: Vec<HlsDefaultsRow> = vec![
        (
            "producer timestamps, no cap",
            "PRODUCER_TIMESTAMP",
            None,
            "NEVER",
            vec![0, 1, 2, 3, 4],
        ),
        (
            "server timestamps, no cap",
            "SERVER_TIMESTAMP",
            None,
            "ALWAYS",
            vec![0, 1, 2, 3, 4],
        ),
        (
            "server timestamps, capped at two",
            "SERVER_TIMESTAMP",
            Some(2),
            "ALWAYS",
            vec![0, 1],
        ),
        (
            "producer timestamps, capped at one",
            "PRODUCER_TIMESTAMP",
            Some(1),
            "NEVER",
            vec![0],
        ),
    ];

    for (case, selector_type, max_results, discontinuity, snapshot) in cases {
        let harness = seeded();
        let mut body = serde_json::json!({
            "StreamName": STREAM,
            "PlaybackMode": "ON_DEMAND",
            "HLSFragmentSelector": {
                "FragmentSelectorType": selector_type,
                "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
            },
        });
        if let Some(max) = max_results {
            body["MaxMediaPlaylistFragmentResults"] = serde_json::json!(max);
        }
        let response = harness.post("/getHLSStreamingSessionURL", &body).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );

        let state = service_state(&*harness.engine).expect("the service store");
        let state = state.lock();
        let session = state
            .sessions
            .values()
            .next()
            .expect("the minted session is recorded");
        assert_eq!(
            session.expires_in_secs, 300,
            "case {case}: the default Expires is 300 seconds, got {session:?}"
        );
        assert_eq!(
            session.container_format, "FRAGMENTED_MP4",
            "case {case}: the default ContainerFormat is FRAGMENTED_MP4, got {session:?}"
        );
        assert_eq!(
            session.display_fragment_timestamp, "NEVER",
            "case {case}: the default DisplayFragmentTimestamp is NEVER, got {session:?}"
        );
        assert_eq!(
            session.discontinuity_mode.as_deref(),
            Some(discontinuity),
            "case {case}: wrong default DiscontinuityMode, got {session:?}"
        );
        assert_eq!(
            session.fragment_numbers,
            snapshot.iter().copied().map(fragment_number).collect::<Vec<_>>(),
            "case {case}: wrong playlist snapshot, got {session:?}"
        );
    }
}

/// RULE: a session that would serve nothing does not exist —
/// `ResourceNotFoundException` (404) for ON_DEMAND / LIVE_REPLAY with no
/// fragment in the requested range, and for LIVE with no fragment in the last
/// 30 seconds. A LIVE session over a freshly ingested archive is minted, and
/// serves the most recent fragments up to the maximum.
#[tokio::test]
async fn get_hls_streaming_session_url_needs_fragments_to_serve() {
    let harness = seeded();
    let empty_range = serde_json::json!({
        "StartTimestamp": T0_SECS - 1000,
        "EndTimestamp": T0_SECS - 1,
    });
    let cases: Vec<(&str, serde_json::Value, u16)> = vec![
        (
            "ON_DEMAND outside the archive",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": empty_range,
                },
            }),
            404,
        ),
        (
            "LIVE_REPLAY outside the archive",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": empty_range,
                },
            }),
            404,
        ),
        (
            "LIVE over an archive older than the live window",
            serde_json::json!({ "StreamName": STREAM, "PlaybackMode": "LIVE" }),
            404,
        ),
        (
            "LIVE over a freshly ingested archive",
            serde_json::json!({ "StreamName": LIVE_STREAM, "PlaybackMode": "LIVE" }),
            200,
        ),
    ];

    for (case, body, status) in cases {
        let response = harness.post("/getHLSStreamingSessionURL", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if status == 404 {
            let reported = body_json(&response);
            assert_eq!(
                reported["__type"], "ResourceNotFoundException",
                "case {case}: expected ResourceNotFoundException, got {reported:?}"
            );
        }
    }

    // The LIVE session that was minted serves the newest fragments, capped by
    // the documented LIVE default of 5.
    let state = service_state(&*harness.engine).expect("the service store");
    let state = state.lock();
    let session = state
        .sessions
        .values()
        .find(|session| session.stream_name == LIVE_STREAM)
        .expect("the LIVE session is recorded");
    let newest: Vec<String> = ((LIVE_FRAGMENTS - 5)..LIVE_FRAGMENTS)
        .map(fragment_number)
        .collect();
    assert_eq!(
        session.fragment_numbers, newest,
        "case LIVE snapshot: a LIVE playlist carries the 5 most recent fragments, got {session:?}"
    );
}

/// RULE: the playback mode decides which selector a session may carry —
/// `HLSFragmentSelector` with a `TimestampRange` is required for ON_DEMAND and
/// LIVE_REPLAY, `EndTimestamp` is required for ON_DEMAND and optional for
/// LIVE_REPLAY, and a LIVE session must carry no `TimestampRange`.
#[tokio::test]
async fn get_hls_streaming_session_url_enforces_the_selector_rules_of_each_mode() {
    let harness = seeded();
    let cases: Vec<(&str, serde_json::Value, u16)> = vec![
        (
            "ON_DEMAND with no selector",
            serde_json::json!({ "StreamName": STREAM, "PlaybackMode": "ON_DEMAND" }),
            400,
        ),
        (
            "LIVE_REPLAY with no selector",
            serde_json::json!({ "StreamName": STREAM, "PlaybackMode": "LIVE_REPLAY" }),
            400,
        ),
        (
            "ON_DEMAND with no EndTimestamp",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            400,
        ),
        (
            "LIVE_REPLAY with no EndTimestamp",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            200,
        ),
        (
            "LIVE with a TimestampRange",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "SERVER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
            400,
        ),
        (
            "ON_DEMAND with an end that precedes the start",
            serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "ON_DEMAND",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS + 10,
                        "EndTimestamp": T0_SECS,
                    },
                },
            }),
            400,
        ),
    ];

    for (case, body, status) in cases {
        let response = harness.post("/getHLSStreamingSessionURL", &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if status == 400 {
            let reported = body_json(&response);
            assert_eq!(
                reported["__type"], "InvalidArgumentException",
                "case {case}: expected InvalidArgumentException, got {reported:?}"
            );
        }
    }
}

/// The widest span the operations whose range documents it admit, in seconds.
///
/// Read off the model, not off a request: `smithy_query(query="doc",
/// shape="HLSTimestampRange", member="EndTimestamp")` — and the same sentence on
/// `DASHTimestampRange$EndTimestamp` and `ClipTimestampRange$EndTimestamp` —
/// states that the end "must be within 24 hours of the specified StartTimestamp,
/// and it must be later than the StartTimestamp value".
const MAX_RANGE_SPAN_SECS: i64 = 24 * 60 * 60;

/// The sentence a range wider than [`MAX_RANGE_SPAN_SECS`] is refused with.
const RANGE_TOO_WIDE: &str = "EndTimestamp must be within 24 hours of StartTimestamp.";

/// The sentence a range whose end is not later than its start is refused with:
/// the other clause of that same documented sentence.
const RANGE_OUT_OF_ORDER: &str = "StartTimestamp must be before EndTimestamp.";

/// One operation whose timestamp range carries the 24-hour sentence: the route,
/// the member its fragment selector is carried in, and the playback mode a
/// request of it states — `None` for `GetClip`, which models no playback mode.
struct RangeOperation {
    path: &'static str,
    selector_member: &'static str,
    playback_mode: Option<&'static str>,
}

/// Every operation of this service whose range documents the 24-hour bound, in
/// each mode that carries a range. `ListFragments` and `GetImages` are absent on
/// purpose: neither range's end carries that sentence.
const RANGE_OPERATIONS: &[RangeOperation] = &[
    RangeOperation {
        path: "/getHLSStreamingSessionURL",
        selector_member: "HLSFragmentSelector",
        playback_mode: Some("ON_DEMAND"),
    },
    RangeOperation {
        path: "/getHLSStreamingSessionURL",
        selector_member: "HLSFragmentSelector",
        playback_mode: Some("LIVE_REPLAY"),
    },
    RangeOperation {
        path: "/getDASHStreamingSessionURL",
        selector_member: "DASHFragmentSelector",
        playback_mode: Some("ON_DEMAND"),
    },
    RangeOperation {
        path: "/getDASHStreamingSessionURL",
        selector_member: "DASHFragmentSelector",
        playback_mode: Some("LIVE_REPLAY"),
    },
    RangeOperation {
        path: "/getClip",
        selector_member: "ClipFragmentSelector",
        playback_mode: None,
    },
];

/// What an operation of [`RANGE_OPERATIONS`] is asked over the range
/// `[T0_SECS, T0_SECS + span_secs]` of the seeded archive, which holds a fragment
/// at its producer start instant `T0_SECS`.
fn range_request(operation: &RangeOperation, span_secs: i64) -> serde_json::Value {
    let mut body = serde_json::json!({ "StreamName": STREAM });
    if let Some(mode) = operation.playback_mode {
        body["PlaybackMode"] = serde_json::json!(mode);
    }
    body[operation.selector_member] = serde_json::json!({
        "FragmentSelectorType": "PRODUCER_TIMESTAMP",
        "TimestampRange": {
            "StartTimestamp": T0_SECS,
            "EndTimestamp": T0_SECS + span_secs,
        },
    });
    body
}

/// What a span is expected to be answered with.
enum SpanExpectation {
    /// The span is one the documented window admits, so the archive answers.
    Admitted,
    /// HTTP 400 `InvalidArgumentException` with exactly this sentence.
    Refused(&'static str),
}

/// How many sessions the service holds.
fn recorded_sessions(harness: &Harness) -> usize {
    let state = service_state(&*harness.engine).expect("the service store");
    let state = state.lock();
    state.sessions.len()
}

/// RULE: a timestamp range whose `EndTimestamp` is documented as having to be
/// "within 24 hours of the specified StartTimestamp" admits a span of at most 24
/// hours — the bound is inclusive — and a wider span is refused
/// `InvalidArgumentException` (400) with one fixed sentence, carrying nothing of
/// the request, before any session or clip exists; the other clause of that same
/// sentence keeps its own refusal for an end that is not later than the start.
///
/// The rows differ along what the rule generalises over: the span (backwards,
/// zero, the narrowest, each side of the published bound, and far past it), the
/// operation whose range carries the sentence, and the playback mode the request
/// states.
#[tokio::test]
async fn a_range_wider_than_the_documented_window_is_refused_in_every_operation_that_bounds_it() {
    let harness = seeded();
    // (case, the span between the two bounds in seconds, what is expected)
    let rows: &[(&str, i64, SpanExpectation)] = &[
        (
            "a day backwards",
            -MAX_RANGE_SPAN_SECS,
            SpanExpectation::Refused(RANGE_OUT_OF_ORDER),
        ),
        (
            "a single instant",
            0,
            SpanExpectation::Refused(RANGE_OUT_OF_ORDER),
        ),
        ("the narrowest span", 1, SpanExpectation::Admitted),
        (
            "one second inside the published bound",
            MAX_RANGE_SPAN_SECS - 1,
            SpanExpectation::Admitted,
        ),
        (
            "exactly the published bound",
            MAX_RANGE_SPAN_SECS,
            SpanExpectation::Admitted,
        ),
        (
            "one second past the published bound",
            MAX_RANGE_SPAN_SECS + 1,
            SpanExpectation::Refused(RANGE_TOO_WIDE),
        ),
        (
            "two days",
            2 * MAX_RANGE_SPAN_SECS,
            SpanExpectation::Refused(RANGE_TOO_WIDE),
        ),
        (
            "thirty days",
            30 * MAX_RANGE_SPAN_SECS,
            SpanExpectation::Refused(RANGE_TOO_WIDE),
        ),
        (
            "a year",
            365 * MAX_RANGE_SPAN_SECS,
            SpanExpectation::Refused(RANGE_TOO_WIDE),
        ),
    ];

    for operation in RANGE_OPERATIONS {
        let mode = operation.playback_mode.unwrap_or("no playback mode");
        for (case, span, expect) in rows {
            let case = format!("{case} over {} in {mode}", operation.path);
            let before = recorded_sessions(&harness);
            let body = range_request(operation, *span);
            let response = harness.post(operation.path, &body).await;
            match expect {
                SpanExpectation::Admitted => {
                    assert_eq!(
                        response.status().as_u16(),
                        200,
                        "case {case}: expected HTTP 200, got {} with body {:?}",
                        response.status(),
                        String::from_utf8_lossy(response.body())
                    );
                }
                SpanExpectation::Refused(sentence) => {
                    assert_eq!(
                        response.status().as_u16(),
                        400,
                        "case {case}: expected HTTP 400, got {} with body {:?}",
                        response.status(),
                        String::from_utf8_lossy(response.body())
                    );
                    let reported = body_json(&response);
                    assert_eq!(
                        reported["__type"], "InvalidArgumentException",
                        "case {case}: expected InvalidArgumentException, got {reported:?}"
                    );
                    let message = reported["Message"]
                        .as_str()
                        .or_else(|| reported["message"].as_str())
                        .unwrap_or_default()
                        .to_string();
                    assert_eq!(
                        message, *sentence,
                        "case {case}: expected {sentence:?}, got {message:?}"
                    );
                    // The sentence is fixed: nothing the request carried — the
                    // stream it addressed, either bound, the span it asked for —
                    // can be read back out of it.
                    for carried in [
                        STREAM.to_string(),
                        T0_SECS.to_string(),
                        (T0_SECS + span).to_string(),
                        span.to_string(),
                    ] {
                        assert!(
                            !message.contains(&carried),
                            "case {case}: the refusal must carry nothing of the request, but \
                             {message:?} contains {carried:?}"
                        );
                    }
                    // A refused request mints nothing: a session is a resource,
                    // so a request the window rule turns away leaves the store
                    // exactly as it was.
                    assert_eq!(
                        recorded_sessions(&harness),
                        before,
                        "case {case}: a refused request must record no session"
                    );
                }
            }
        }
    }
}

/// RULE: a stream that retains no data cannot have a streaming session —
/// `NoDataRetentionException` (400).
#[tokio::test]
async fn get_hls_streaming_session_url_refuses_a_stream_with_no_retention() {
    let harness = seeded();
    let body = serde_json::json!({
        "StreamName": NO_RETENTION_STREAM,
        "PlaybackMode": "ON_DEMAND",
        "HLSFragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
        },
    });
    let response = harness.post("/getHLSStreamingSessionURL", &body).await;
    assert_eq!(
        response.status().as_u16(),
        400,
        "expected HTTP 400, got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let reported = body_json(&response);
    assert_eq!(
        reported["__type"], "NoDataRetentionException",
        "expected NoDataRetentionException, got {reported:?}"
    );
}

/// Seed `stream` with one fragment per entry of `fragments`, ten seconds apart
/// from [`T0_SECS`], registered and retaining data like every other seeded
/// stream — the archive a probe of one read path is driven over.
fn seed_probe_stream(harness: &Harness, stream: &str, fragments: &[Vec<u8>]) {
    let state = service_state(&*harness.engine).expect("the service store");
    let mut state = state.lock();
    let archived = ArchivedStream::new(stream, REGION, from_epoch_millis(T0_SECS * 1000), 24);
    register_stream(&harness.engine, &archived);
    state.put_stream(archived);
    for (index, media) in fragments.iter().enumerate() {
        let at = from_epoch_millis((T0_SECS + 10 * index as i64) * 1000);
        assert!(
            state.append_fragment(
                stream,
                StoredFragment::new(fragment_number(index as u64), at, at, 2000, media.clone()),
            ),
            "case seed {stream:?} fragment {index}: the stream must exist"
        );
    }
}

/// Seed `stream` with [`seed_probe_stream`] and answer the request body that
/// clips the whole of it.
fn seeded_clip_request(
    harness: &Harness,
    stream: &str,
    fragments: &[Vec<u8>],
) -> serde_json::Value {
    seed_probe_stream(harness, stream, fragments);
    serde_json::json!({
        "StreamName": stream,
        "ClipFragmentSelector": {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": {
                "StartTimestamp": T0_SECS,
                "EndTimestamp": T0_SECS + 1000,
            },
        },
    })
}

/// One row of the mux table: the fragments an archive holds, and the track the
/// clip of them has to carry.
struct MuxRow {
    case: &'static str,
    fragments: Vec<MkvFragment>,
    /// The samples the clip's track must carry, in order.
    expected_samples: Vec<Vec<u8>>,
    /// The geometry the clip's track must state.
    geometry: (u16, u16),
    /// The parameter sets its sample description must carry.
    parameter_sets: (Vec<u8>, Vec<u8>),
}

/// RULE: whatever shape the archived Matroska fragments have, the clip is an MP4
/// file whose one video track carries their H.264 track's frames as its samples,
/// with that track's codec private data as the sample description and the coded
/// geometry it states.
///
/// The rows differ along what the rule generalises over: how many fragments the
/// range holds and how many frames each carries, which spelling of the H.264
/// codec id the track declares, which track number it is, where the geometry is
/// declared, the container's own timecode scale, and the framing the producer
/// wrote its units in.
#[tokio::test]
async fn a_clip_is_an_mp4_of_the_frames_the_archived_track_carries() {
    let coded = coded_video();
    let fixture_sets = || (coded.seq_param_set.clone(), coded.pic_param_set.clone());
    let coded_geometry = (
        u16::try_from(CODED_WIDTH).expect("a short side"),
        u16::try_from(CODED_HEIGHT).expect("a short side"),
    );

    // A track whose record carries parameter sets that are not readable H.264,
    // for the row where the CONTAINER is what declares the geometry.
    let opaque_seq: Vec<u8> = vec![0x67, 0x64, 0x00, 0x1F, 0x00, 0x00];
    let opaque_pic: Vec<u8> = vec![0x68, 0xEE, 0x3C, 0x80];

    // A producer that wrote its units DELIMITED rather than length-prefixed: the
    // same access units, as the Annex B byte stream (ISO/IEC 14496-10 Annex B).
    let delimited = |unit: &[u8]| {
        let mut stream = Vec::new();
        assert!(
            ::service_common::h264_mp4::annex_b_from_avcc(unit, 4, &mut stream),
            "case the delimited fixture: the access unit must re-frame"
        );
        stream
    };

    let mut rows: Vec<MuxRow> = vec![
        MuxRow {
            case: "one fragment of one frame",
            fragments: vec![MkvFragment::of(0, 1)],
            expected_samples: access_units(0, 1),
            geometry: coded_geometry,
            parameter_sets: fixture_sets(),
        },
        MuxRow {
            case: "one fragment of three frames",
            fragments: vec![MkvFragment::of(2, 3)],
            expected_samples: access_units(2, 3),
            geometry: coded_geometry,
            parameter_sets: fixture_sets(),
        },
        MuxRow {
            case: "three fragments of two frames each",
            fragments: vec![
                MkvFragment::of(0, 2),
                MkvFragment::of(2, 2),
                MkvFragment::of(4, 2),
            ],
            expected_samples: access_units(0, 6),
            geometry: coded_geometry,
            parameter_sets: fixture_sets(),
        },
        MuxRow {
            case: "the codec id spelling this operation's documentation prints",
            fragments: vec![MkvFragment {
                codec_id: "V_MPEG/ISO/AVC".to_string(),
                ..MkvFragment::of(5, 2)
            }],
            expected_samples: access_units(5, 2),
            geometry: coded_geometry,
            parameter_sets: fixture_sets(),
        },
        MuxRow {
            case: "a track number other than one",
            fragments: vec![MkvFragment {
                track: 3,
                ..MkvFragment::of(7, 2)
            }],
            expected_samples: access_units(7, 2),
            geometry: coded_geometry,
            parameter_sets: fixture_sets(),
        },
        MuxRow {
            case: "a container that declares the geometry as well",
            fragments: vec![MkvFragment {
                pixel_size: Some((u64::from(CODED_WIDTH), u64::from(CODED_HEIGHT))),
                ..MkvFragment::of(1, 2)
            }],
            expected_samples: access_units(1, 2),
            geometry: coded_geometry,
            parameter_sets: fixture_sets(),
        },
        MuxRow {
            case: "a record whose parameter sets are opaque, with the container's geometry",
            fragments: vec![MkvFragment {
                codec_private: Some(avc_record(&opaque_seq, &opaque_pic)),
                pixel_size: Some((320, 240)),
                ..MkvFragment::of(3, 2)
            }],
            expected_samples: access_units(3, 2),
            geometry: (320, 240),
            parameter_sets: (opaque_seq.clone(), opaque_pic.clone()),
        },
        MuxRow {
            case: "a hundred-microsecond timecode scale",
            fragments: vec![MkvFragment {
                scale_nanos: 100_000,
                timecode: 4_000,
                spacing_ticks: 330,
                ..MkvFragment::of(6, 3)
            }],
            expected_samples: access_units(6, 3),
            geometry: coded_geometry,
            parameter_sets: fixture_sets(),
        },
        MuxRow {
            case: "frames the producer wrote as a delimited byte stream",
            fragments: vec![MkvFragment {
                frames: access_units(8, 2)
                    .iter()
                    .map(|unit| delimited(unit))
                    .collect(),
                ..MkvFragment::of(8, 2)
            }],
            expected_samples: access_units(8, 2),
            geometry: coded_geometry,
            parameter_sets: fixture_sets(),
        },
    ];

    let harness = seeded();
    for (index, row) in rows.drain(..).enumerate() {
        let stream = format!("mux-probe-{index}");
        let media: Vec<Vec<u8>> = row
            .fragments
            .iter()
            .map(MkvFragment::bytes)
            .collect();
        let body = seeded_clip_request(&harness, &stream, &media);
        let response = harness.post("/getClip", &body).await;
        let case = row.case;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        assert_eq!(
            content_type(&response),
            "video/mp4",
            "case {case}: GetClip downloads an MP4 clip"
        );
        assert_is_mp4(case, response.body());
        let track = clip_track(case, response.body());
        assert_eq!(
            track.samples,
            row.expected_samples,
            "case {case}: expected the archived frames as the clip's samples, got {} samples",
            track.samples.len()
        );
        assert_eq!(
            (track.width, track.height),
            row.geometry,
            "case {case}: expected the track to state {:?}, got {}x{}",
            row.geometry,
            track.width,
            track.height
        );
        assert_eq!(
            (track.seq_param_set, track.pic_param_set),
            row.parameter_sets,
            "case {case}: the sample description must carry the track's own parameter sets"
        );
        assert_eq!(
            track.sync.first(),
            Some(&true),
            "case {case}: a frame the producer marked a keyframe must be a sync sample, got {:?}",
            track.sync
        );
    }
}

/// RULE: the mux has preconditions, and each is the modelled refusal for it — a
/// track that declares no codec private data, codec private data that is not a
/// decoder configuration record, media whose codec ids name no H.264 video track,
/// and fragments whose frames cannot be read at all.
///
/// The rows differ along which precondition the archived media breaks, and along
/// which track and which fragment of the range breaks it.
#[tokio::test]
async fn get_clip_refuses_archived_media_it_cannot_mux() {
    // (case, the media the archive holds, the code, the message)
    let rows: Vec<(&str, Vec<Vec<u8>>, &str, String)> = vec![
        (
            "a track that declares no codec private data",
            vec![MkvFragment {
                codec_private: None,
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "MissingCodecPrivateDataException",
            "Missing codec private data in fragment for track 1.".to_string(),
        ),
        (
            "another track that declares none",
            vec![MkvFragment {
                track: 2,
                codec_private: None,
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "MissingCodecPrivateDataException",
            "Missing codec private data in fragment for track 2.".to_string(),
        ),
        (
            "codec private data of zero length",
            vec![MkvFragment {
                codec_private: Some(Vec::new()),
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "MissingCodecPrivateDataException",
            "Missing codec private data in fragment for track 1.".to_string(),
        ),
        (
            "a later fragment of the range that declares none",
            vec![
                MkvFragment::of(0, 2).bytes(),
                MkvFragment {
                    codec_private: None,
                    ..MkvFragment::of(2, 2)
                }
                .bytes(),
            ],
            "MissingCodecPrivateDataException",
            "Missing codec private data in fragment for track 1.".to_string(),
        ),
        (
            "codec private data that is not a record at all",
            vec![MkvFragment {
                codec_private: Some(b"not-a-decoder-configuration-record".to_vec()),
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "InvalidCodecPrivateDataException",
            "Invalid codec private data in fragment for track 1.".to_string(),
        ),
        (
            "a record of another configuration version",
            vec![MkvFragment {
                codec_private: Some({
                    let mut record = fixture_record();
                    record[0] = 2;
                    record
                }),
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "InvalidCodecPrivateDataException",
            "Invalid codec private data in fragment for track 1.".to_string(),
        ),
        (
            "a record whose parameter set runs past its own bytes",
            vec![MkvFragment {
                codec_private: Some(fixture_record()[..8].to_vec()),
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "InvalidCodecPrivateDataException",
            "Invalid codec private data in fragment for track 1.".to_string(),
        ),
        (
            "a record whose sequence parameter set states no profile",
            vec![MkvFragment {
                codec_private: Some(avc_record(&[0x67, 0x64], &[0x68, 0xEE, 0x3C, 0x80])),
                pixel_size: Some((320, 240)),
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "InvalidCodecPrivateDataException",
            "Invalid codec private data in fragment for track 1.".to_string(),
        ),
        (
            "a record stating neither a readable parameter set nor a geometry",
            vec![MkvFragment {
                codec_private: Some(avc_record(
                    &[0x67, 0x64, 0x00, 0x1F, 0x00, 0x00],
                    &[0x68, 0xEE, 0x3C, 0x80],
                )),
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "InvalidCodecPrivateDataException",
            "Invalid codec private data in fragment for track 1.".to_string(),
        ),
        (
            "an audio codec id",
            vec![MkvFragment {
                codec_id: "A_AAC".to_string(),
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "UnsupportedStreamMediaTypeException",
            UNSUPPORTED_MEDIA_TYPE.to_string(),
        ),
        (
            "a video codec id that is not H.264",
            vec![MkvFragment {
                codec_id: "V_VP9".to_string(),
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "UnsupportedStreamMediaTypeException",
            UNSUPPORTED_MEDIA_TYPE.to_string(),
        ),
        (
            "media that is not a Matroska document",
            vec![b"this is not a matroska document".to_vec()],
            "InvalidMediaFrameException",
            INVALID_MEDIA_FRAME.to_string(),
        ),
        (
            "a fragment that carries no frame",
            vec![MkvFragment {
                frames: Vec::new(),
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "InvalidMediaFrameException",
            INVALID_MEDIA_FRAME.to_string(),
        ),
    ];

    let harness = seeded();
    for (index, (case, media, code, message)) in rows.into_iter().enumerate() {
        let stream = format!("refusal-probe-{index}");
        let body = seeded_clip_request(&harness, &stream, &media);
        let response = harness.post("/getClip", &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        assert_eq!(
            reported["__type"], code,
            "case {case}: expected {code}, got {reported:?}"
        );
        assert_eq!(
            reported["Message"].as_str().unwrap_or_default(),
            message,
            "case {case}: expected {message:?}, got {reported:?}"
        );
    }
}

/// The sentence a clip of media whose codec ids name no H.264 video track is
/// refused with: the modelled description of `UnsupportedStreamMediaTypeException`.
const UNSUPPORTED_MEDIA_TYPE: &str =
    "The type of the media (for example, h.264 or h.265 video or ACC or G.711 audio) could not \
     be determined from the codec IDs of the tracks in the first fragment for a playback session.";

/// The two streaming-session operations: the route, the input member each
/// carries its fragment selector in, and the output member each reports its URL
/// in (`smithy_query(query="operation",operation="GetHLSStreamingSessionURL,\
/// GetDASHStreamingSessionURL")`).
const SESSION_PROTOCOLS: [(&str, &str, &str); 2] = [
    (
        "/getHLSStreamingSessionURL",
        "HLSFragmentSelector",
        "HLSStreamingSessionURL",
    ),
    (
        "/getDASHStreamingSessionURL",
        "DASHFragmentSelector",
        "DASHStreamingSessionURL",
    ),
];

/// The body of an ON_DEMAND session request over the whole of `stream`, with the
/// selector in the member `member` names.
fn probe_session_request(stream: &str, member: &str) -> serde_json::Value {
    serde_json::json!({
        "StreamName": stream,
        "PlaybackMode": "ON_DEMAND",
        member: {
            "FragmentSelectorType": "PRODUCER_TIMESTAMP",
            "TimestampRange": {
                "StartTimestamp": T0_SECS,
                "EndTimestamp": T0_SECS + 1000,
            },
        },
    })
}

/// RULE: a streaming session exists only over media that meets the requirements
/// its operation documents — the codec ids of the fragment it starts from must
/// name the H.264 video track, and that track must carry codec private data that
/// reads as an AVC decoder configuration record — and media that meets neither is
/// the modelled refusal for the requirement it breaks (400), the same refusal
/// `GetClip` answers for the same archive.
///
/// The rows differ along which requirement the media breaks, the track and the
/// codec id it breaks it on, and whether the container reads at all; every row is
/// driven through BOTH session operations, because the requirement is a property
/// of the media rather than of the streaming protocol.
#[tokio::test]
async fn a_streaming_session_refuses_media_that_breaks_the_codec_requirements() {
    // (case, the media the archive holds, the error code, the message)
    let rows: Vec<(&str, Vec<Vec<u8>>, &str, String)> = vec![
        (
            "a track that declares no codec private data",
            vec![MkvFragment {
                codec_private: None,
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "MissingCodecPrivateDataException",
            "Missing codec private data in fragment for track 1.".to_string(),
        ),
        (
            "another track that declares none",
            vec![MkvFragment {
                track: 4,
                codec_private: None,
                ..MkvFragment::of(1, 2)
            }
            .bytes()],
            "MissingCodecPrivateDataException",
            "Missing codec private data in fragment for track 4.".to_string(),
        ),
        (
            "codec private data of zero length",
            vec![MkvFragment {
                codec_private: Some(Vec::new()),
                ..MkvFragment::of(2, 2)
            }
            .bytes()],
            "MissingCodecPrivateDataException",
            "Missing codec private data in fragment for track 1.".to_string(),
        ),
        (
            "codec private data that is not a record at all",
            vec![MkvFragment {
                codec_private: Some(b"not-a-decoder-configuration-record".to_vec()),
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "InvalidCodecPrivateDataException",
            "Invalid codec private data in fragment for track 1.".to_string(),
        ),
        (
            "a record of another configuration version, on another track",
            vec![MkvFragment {
                track: 2,
                codec_private: Some({
                    let mut record = fixture_record();
                    record[0] = 2;
                    record
                }),
                ..MkvFragment::of(3, 2)
            }
            .bytes()],
            "InvalidCodecPrivateDataException",
            "Invalid codec private data in fragment for track 2.".to_string(),
        ),
        (
            "a record stating neither a readable parameter set nor a geometry",
            vec![MkvFragment {
                codec_private: Some(avc_record(
                    &[0x67, 0x64, 0x00, 0x1F, 0x00, 0x00],
                    &[0x68, 0xEE, 0x3C, 0x80],
                )),
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "InvalidCodecPrivateDataException",
            "Invalid codec private data in fragment for track 1.".to_string(),
        ),
        (
            "a track that declares no codec id at all",
            vec![MkvFragment {
                codec_id: String::new(),
                ..MkvFragment::of(0, 2)
            }
            .bytes()],
            "UnsupportedStreamMediaTypeException",
            UNSUPPORTED_MEDIA_TYPE.to_string(),
        ),
        (
            "a video codec id of a codec this read path does not read",
            vec![MkvFragment {
                codec_id: "V_VP9".to_string(),
                ..MkvFragment::of(4, 2)
            }
            .bytes()],
            "UnsupportedStreamMediaTypeException",
            UNSUPPORTED_MEDIA_TYPE.to_string(),
        ),
        (
            "media that is not a Matroska document, so it names no codec id",
            vec![b"this is not a matroska document".to_vec()],
            "UnsupportedStreamMediaTypeException",
            UNSUPPORTED_MEDIA_TYPE.to_string(),
        ),
    ];

    let harness = seeded();
    for (path, member, _) in SESSION_PROTOCOLS {
        for (index, (row, media, code, message)) in rows.iter().enumerate() {
            let stream = format!("{}-media-probe-{index}", path.trim_start_matches('/'));
            seed_probe_stream(&harness, &stream, media);
            let response = harness.post(path, &probe_session_request(&stream, member)).await;
            let case = format!("{path}, {row}");
            assert_eq!(
                response.status().as_u16(),
                400,
                "case {case}: expected HTTP 400, got {} with body {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            let reported = body_json(&response);
            assert_eq!(
                reported["__type"], *code,
                "case {case}: expected {code}, got {reported:?}"
            );
            assert_eq!(
                reported["Message"].as_str().unwrap_or_default(),
                message.as_str(),
                "case {case}: expected {message:?}, got {reported:?}"
            );
            assert_eq!(
                recorded_sessions(&harness),
                0,
                "case {case}: a refused request must mint no session"
            );
        }
    }
}

/// RULE, from the other side: the requirements are read off the fragment the
/// session STARTS from — "the codec IDs of the tracks in the first fragment for a
/// playback session" — so a session whose first fragment meets them is minted and
/// reports its URL, whatever a later fragment of the same range declares.
#[tokio::test]
async fn a_streaming_session_reads_the_requirements_off_the_fragment_it_starts_from() {
    let harness = seeded();
    for (path, member, url_member) in SESSION_PROTOCOLS {
        let stream = format!("{}-first-fragment", path.trim_start_matches('/'));
        seed_probe_stream(
            &harness,
            &stream,
            &[
                MkvFragment::of(0, 2).bytes(),
                MkvFragment {
                    codec_private: None,
                    ..MkvFragment::of(2, 2)
                }
                .bytes(),
            ],
        );
        let response = harness.post(path, &probe_session_request(&stream, member)).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {path}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let url = body_json(&response)[url_member]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            url.contains("SessionToken="),
            "case {path}: expected a session URL carrying a token, got {url:?}"
        );
    }
}

/// The sentence a clip of media whose frames cannot be read is refused with: the
/// modelled description of `InvalidMediaFrameException`.
const INVALID_MEDIA_FRAME: &str =
    "One or more frames in the requested clip could not be parsed based on the specified codec.";

/// Every operation of this service is implemented, so none of them answers
/// 501: `GetDASHStreamingSessionURL`, the last scaffold, is exercised in depth
/// by the `get_dash_streaming_session_url` suite.
#[tokio::test]
async fn no_operation_of_this_service_answers_not_implemented() {
    let harness = seeded();
    let named = serde_json::json!({ "StreamName": STREAM });

    for (path, body) in read_loop_operations() {
        let response = harness.post(path, &with_members(&body, &named)).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {path}: expected HTTP 200, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
    }
}

// ---------------------------------------------------------------------------
// The playback plane: what the streaming-session URL resolves to.
// ---------------------------------------------------------------------------

/// The media type an HLS playlist is served as.
const HLS_PLAYLIST_TYPE: &str = "application/vnd.apple.mpegurl";
/// The media type an MPEG-DASH manifest is served as.
const DASH_MANIFEST_TYPE: &str = "application/dash+xml";
/// The media type an MP4 initialization or media fragment is served as.
const MP4_FRAGMENT_TYPE: &str = "video/mp4";
/// The media type an MPEG TS fragment is served as.
const TS_FRAGMENT_TYPE: &str = "video/MP2T";
/// How many bytes one MPEG-2 transport packet is (ISO/IEC 13818-1).
const TS_PACKET_BYTES: usize = 188;

/// A session over the whole seeded archive of [`STREAM`], by producer timestamps.
fn on_demand_body(extra: &serde_json::Value, selector_member: &str) -> serde_json::Value {
    with_members(
        &serde_json::json!({
            "StreamName": STREAM,
            "PlaybackMode": "ON_DEMAND",
            selector_member: {
                "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                "TimestampRange": { "StartTimestamp": T0_SECS, "EndTimestamp": T0_SECS + 1000 },
            },
        }),
        extra,
    )
}

/// Mint a session through `path` and answer the URL it reported.
async fn minted_session_url(
    harness: &Harness,
    case: &str,
    path: &str,
    member: &str,
    body: &serde_json::Value,
) -> String {
    let response = harness.post(path, body).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case {case}: expected HTTP 200 from {path}, got {} with body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let url = body_json(&response)[member]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !url.is_empty(),
        "case {case}: {path} must report a {member}, got {:?}",
        String::from_utf8_lossy(response.body())
    );
    url
}

/// The body of a response as text.
fn text_body(response: &http::Response<Bytes>) -> String {
    String::from_utf8_lossy(response.body()).into_owned()
}

/// The `content-type` of a response.
fn served_type(response: &http::Response<Bytes>) -> String {
    content_type(response)
}

/// The URL of `relative`, resolved against the playback URL `base` the way a
/// media player resolves a reference inside a playlist or a manifest.
fn sibling_url(base: &str, relative: &str) -> String {
    let (prefix, _) = base
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("the playback URL {base:?} carries a path"));
    format!("{prefix}/{relative}")
}

/// Every resource a playlist or a manifest REFERENCES: the segment lines and
/// `EXT-X-MAP` of an HLS playlist, the `sourceURL` and `media` attributes of an
/// MPEG-DASH manifest, each with its XML entities resolved.
fn referenced_resources(document: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in document.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXT-X-MAP:URI=\"") {
            if let Some(uri) = rest.split('"').next() {
                found.push(uri.to_string());
            }
            continue;
        }
        if line.starts_with('#') || line.starts_with('<') {
            for attribute in ["sourceURL=\"", "media=\""] {
                if let Some((_, rest)) = line.split_once(attribute) {
                    if let Some(uri) = rest.split('"').next() {
                        found.push(uri.replace("&amp;", "&"));
                    }
                }
            }
            continue;
        }
        found.push(line.to_string());
    }
    found
}

/// The instant the `index`-th seeded fragment's PRODUCER timestamp names, in the
/// form a playlist's wall-clock tag and a manifest's timestamp attribute carry.
fn reported_producer_instant(index: u64) -> String {
    archived_media::state::to_date_time(from_epoch_millis(producer_secs(index) * 1000))
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("the instant renders")
}

/// One row of the playback walk: a session to mint, and the root resource its URL
/// must resolve to.
struct WalkRow {
    case: &'static str,
    path: &'static str,
    member: &'static str,
    body: serde_json::Value,
    root_type: &'static str,
    /// The media type every LEAF the session's documents name is served as.
    leaf_type: &'static str,
}

/// RULE: a URL an operation reports for the caller to FETCH is served by this
/// emulator at the address it reported, and so is every resource the document at
/// that address names — down to the media bytes.
///
/// The rows differ along what the rule generalises over: the playback protocol
/// (HLS, MPEG-DASH), the container the media is packaged in (`FRAGMENTED_MP4`,
/// `MPEG_TS`), the playback mode (`ON_DEMAND`, `LIVE`) and the stream whose
/// archive is played.
#[tokio::test]
async fn a_streaming_session_url_is_served_down_to_its_media() {
    let rows: Vec<WalkRow> = vec![
        WalkRow {
            case: "an HLS session of MP4 fragments",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: on_demand_body(&serde_json::json!({}), "HLSFragmentSelector"),
            root_type: HLS_PLAYLIST_TYPE,
            leaf_type: MP4_FRAGMENT_TYPE,
        },
        WalkRow {
            case: "an HLS session of MPEG TS fragments",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: on_demand_body(
                &serde_json::json!({ "ContainerFormat": "MPEG_TS" }),
                "HLSFragmentSelector",
            ),
            root_type: HLS_PLAYLIST_TYPE,
            leaf_type: TS_FRAGMENT_TYPE,
        },
        WalkRow {
            case: "a LIVE HLS session over the stream ingested just now",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: serde_json::json!({ "StreamName": LIVE_STREAM, "PlaybackMode": "LIVE" }),
            root_type: HLS_PLAYLIST_TYPE,
            leaf_type: MP4_FRAGMENT_TYPE,
        },
        WalkRow {
            case: "an MPEG-DASH session",
            path: "/getDASHStreamingSessionURL",
            member: "DASHStreamingSessionURL",
            body: on_demand_body(&serde_json::json!({}), "DASHFragmentSelector"),
            root_type: DASH_MANIFEST_TYPE,
            leaf_type: MP4_FRAGMENT_TYPE,
        },
    ];

    let harness = seeded();
    for row in rows {
        let case = row.case;
        let url =
            minted_session_url(&harness, case, row.path, row.member, &row.body).await;
        assert!(
            url.starts_with(&format!("https://{EMULATOR_ENDPOINT}/")),
            "case {case}: the session URL must address this emulator, got {url:?}"
        );

        let root = harness.fetch(&url).await;
        assert_eq!(
            root.status().as_u16(),
            200,
            "case {case}: the reported URL {url:?} must be served, got {} {:?}",
            root.status(),
            text_body(&root)
        );
        let actual_type = served_type(&root);
        assert_eq!(
            actual_type, row.root_type,
            "case {case}: expected the root resource as {:?}, got {actual_type:?}",
            row.root_type
        );

        let root_document = text_body(&root);
        let children = referenced_resources(&root_document);
        assert!(
            !children.is_empty(),
            "case {case}: the root resource must name the resources below it, got \
             {root_document:?}"
        );
        // A document that names resources is walked one level further, so the walk
        // reaches the MEDIA of the session rather than stopping at a playlist that
        // names it.
        let mut leaves = 0usize;
        for child in children {
            let child_url = sibling_url(&url, &child);
            let response = harness.fetch(&child_url).await;
            assert_eq!(
                response.status().as_u16(),
                200,
                "case {case}: the resource {child:?} the session names must be served, got {} {:?}",
                response.status(),
                text_body(&response)
            );
            assert!(
                !response.body().is_empty(),
                "case {case}: the resource {child:?} must carry a body"
            );
            let child_type = served_type(&response);
            if child_type == HLS_PLAYLIST_TYPE || child_type == DASH_MANIFEST_TYPE {
                let document = text_body(&response);
                let grandchildren = referenced_resources(&document);
                assert!(
                    !grandchildren.is_empty(),
                    "case {case}: the playlist {child:?} must name its media, got {document:?}"
                );
                for grandchild in grandchildren {
                    let leaf_url = sibling_url(&url, &grandchild);
                    let leaf = harness.fetch(&leaf_url).await;
                    assert_eq!(
                        leaf.status().as_u16(),
                        200,
                        "case {case}: the media {grandchild:?} must be served, got {} {:?}",
                        leaf.status(),
                        text_body(&leaf)
                    );
                    assert!(
                        !leaf.body().is_empty(),
                        "case {case}: the media {grandchild:?} must carry bytes"
                    );
                    let leaf_type = served_type(&leaf);
                    let expected = match grandchild.contains("InitFragment") {
                        true => MP4_FRAGMENT_TYPE,
                        false => row.leaf_type,
                    };
                    assert_eq!(
                        leaf_type, expected,
                        "case {case}: expected {grandchild:?} as {expected:?}, got {leaf_type:?}"
                    );
                    leaves += 1;
                }
            } else {
                leaves += 1;
            }
        }
        assert!(
            leaves > 0,
            "case {case}: the walk must reach the session's media"
        );
    }
}

/// One row of the HLS playlist table: a session parameter, and what the media
/// playlist it is minted with must state.
struct PlaylistRow {
    case: &'static str,
    /// The members added to the minting request.
    extra: serde_json::Value,
    /// The stream and mode the session plays, when they are not the seeded
    /// on-demand pair.
    live: bool,
    /// How many discontinuity markers the playlist carries.
    discontinuities: usize,
    /// How many wall-clock tags it carries.
    timestamps: usize,
    /// The resource its segments are fetched at.
    segment: &'static str,
    /// Whether it names an initialization fragment of its own.
    init: bool,
    /// How many segments it lists.
    segments: usize,
}

/// RULE: the media playlist of a session states the parameters that session was
/// minted with — the container its segments are packaged in, where
/// discontinuities are marked, whether fragment start timestamps are shown, and
/// how many fragments the playlist carries.
///
/// The rows differ along what the rule generalises over: each of the three
/// `DiscontinuityMode` values, both `DisplayFragmentTimestamp` values, both
/// `ContainerFormat` values, a capped and an uncapped playlist, and an archive
/// whose fragments are contiguous beside one whose fragments have gaps — which is
/// what separates `ON_DISCONTINUITY` from `ALWAYS`.
#[tokio::test]
async fn an_hls_session_playlist_states_the_parameters_it_was_minted_with() {
    // The seeded on-demand archive holds 5 fragments 10 seconds apart, each 2
    // seconds long: every boundary is a gap of 8 seconds. The live archive holds 7
    // fragments one second apart, each one second long: its boundaries are
    // contiguous, and a LIVE session serves the newest 5 of them.
    let rows: Vec<PlaylistRow> = vec![
        PlaylistRow {
            case: "the defaults over producer timestamps",
            extra: serde_json::json!({}),
            live: false,
            discontinuities: 0,
            timestamps: 0,
            segment: "getMP4MediaFragment.mp4",
            init: true,
            segments: 5,
        },
        PlaylistRow {
            case: "discontinuity markers between every fragment",
            extra: serde_json::json!({ "DiscontinuityMode": "ALWAYS" }),
            live: false,
            discontinuities: 4,
            timestamps: 0,
            segment: "getMP4MediaFragment.mp4",
            init: true,
            segments: 5,
        },
        PlaylistRow {
            case: "markers only where the timeline breaks, over an archive with gaps",
            extra: serde_json::json!({ "DiscontinuityMode": "ON_DISCONTINUITY" }),
            live: false,
            discontinuities: 4,
            timestamps: 0,
            segment: "getMP4MediaFragment.mp4",
            init: true,
            segments: 5,
        },
        PlaylistRow {
            case: "markers only where the timeline breaks, over a contiguous archive",
            extra: serde_json::json!({ "DiscontinuityMode": "ON_DISCONTINUITY" }),
            live: true,
            discontinuities: 0,
            timestamps: 0,
            segment: "getMP4MediaFragment.mp4",
            init: true,
            segments: 5,
        },
        PlaylistRow {
            case: "markers between every fragment of a contiguous archive",
            extra: serde_json::json!({ "DiscontinuityMode": "ALWAYS" }),
            live: true,
            discontinuities: 4,
            timestamps: 0,
            segment: "getMP4MediaFragment.mp4",
            init: true,
            segments: 5,
        },
        PlaylistRow {
            case: "fragment start timestamps shown",
            extra: serde_json::json!({ "DisplayFragmentTimestamp": "ALWAYS" }),
            live: false,
            discontinuities: 0,
            timestamps: 5,
            segment: "getMP4MediaFragment.mp4",
            init: true,
            segments: 5,
        },
        PlaylistRow {
            case: "fragment start timestamps not shown",
            extra: serde_json::json!({ "DisplayFragmentTimestamp": "NEVER" }),
            live: false,
            discontinuities: 0,
            timestamps: 0,
            segment: "getMP4MediaFragment.mp4",
            init: true,
            segments: 5,
        },
        PlaylistRow {
            case: "media packaged as MPEG TS",
            extra: serde_json::json!({ "ContainerFormat": "MPEG_TS" }),
            live: false,
            discontinuities: 0,
            timestamps: 0,
            segment: "getTSFragment.ts",
            init: false,
            segments: 5,
        },
        PlaylistRow {
            case: "a playlist capped at two fragments",
            extra: serde_json::json!({ "MaxMediaPlaylistFragmentResults": 2 }),
            live: false,
            discontinuities: 0,
            timestamps: 0,
            segment: "getMP4MediaFragment.mp4",
            init: true,
            segments: 2,
        },
    ];

    let harness = seeded();
    for row in rows {
        let case = row.case;
        let body = match row.live {
            true => with_members(
                &serde_json::json!({ "StreamName": LIVE_STREAM, "PlaybackMode": "LIVE" }),
                &row.extra,
            ),
            false => on_demand_body(&row.extra, "HLSFragmentSelector"),
        };
        let url = minted_session_url(
            &harness,
            case,
            "/getHLSStreamingSessionURL",
            "HLSStreamingSessionURL",
            &body,
        )
        .await;

        let master = harness.fetch(&url).await;
        let master_document = text_body(&master);
        let variant = referenced_resources(&master_document)
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                panic!("case {case}: the master playlist must name a media playlist")
            });
        let media = harness.fetch(&sibling_url(&url, &variant)).await;
        assert_eq!(
            media.status().as_u16(),
            200,
            "case {case}: the media playlist must be served, got {}",
            media.status()
        );
        let playlist = text_body(&media);

        let discontinuities = playlist
            .lines()
            .filter(|line| line.trim() == "#EXT-X-DISCONTINUITY")
            .count();
        assert_eq!(
            discontinuities, row.discontinuities,
            "case {case}: expected {} discontinuity markers, got {discontinuities} in \
             {playlist:?}",
            row.discontinuities
        );

        let timestamps = playlist
            .lines()
            .filter(|line| line.trim().starts_with("#EXT-X-PROGRAM-DATE-TIME:"))
            .count();
        assert_eq!(
            timestamps, row.timestamps,
            "case {case}: expected {} wall-clock tags, got {timestamps} in {playlist:?}",
            row.timestamps
        );

        let segments: Vec<String> = playlist
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_string)
            .collect();
        assert_eq!(
            segments.len(),
            row.segments,
            "case {case}: expected {} segments, got {segments:?}",
            row.segments
        );
        for segment in &segments {
            assert!(
                segment.starts_with(row.segment),
                "case {case}: expected segments at {:?}, got {segment:?}",
                row.segment
            );
        }
        let names_init = playlist.contains("#EXT-X-MAP:URI=");
        assert_eq!(
            names_init, row.init,
            "case {case}: expected an initialization fragment named: {}, got {playlist:?}",
            row.init
        );
    }
}

/// RULE (the other half of the wall-clock tag): the timestamp a playlist reports
/// for a fragment is the one the session's own fragment selector reads — "when
/// HLSFragmentSelector is PRODUCER_TIMESTAMP, the timestamps will be the producer
/// start timestamps" — and it is the archive's value for that fragment, not the
/// instant the playlist was fetched.
#[tokio::test]
async fn a_shown_fragment_timestamp_is_the_archives_own() {
    let harness = seeded();
    let url = minted_session_url(
        &harness,
        "producer timestamps shown",
        "/getHLSStreamingSessionURL",
        "HLSStreamingSessionURL",
        &on_demand_body(
            &serde_json::json!({ "DisplayFragmentTimestamp": "ALWAYS" }),
            "HLSFragmentSelector",
        ),
    )
    .await;
    let master = harness.fetch(&url).await;
    let variant = referenced_resources(&text_body(&master))
        .into_iter()
        .next()
        .expect("the master playlist names a media playlist");
    let playlist = text_body(&harness.fetch(&sibling_url(&url, &variant)).await);

    let shown: Vec<String> = playlist
        .lines()
        .filter_map(|line| line.trim().strip_prefix("#EXT-X-PROGRAM-DATE-TIME:"))
        .map(str::to_string)
        .collect();
    let expected: Vec<String> = (0..SEEDED_FRAGMENTS).map(reported_producer_instant).collect();
    assert_eq!(
        shown, expected,
        "case the seeded archive: expected the producer timestamps {expected:?}, got {shown:?}"
    );
}

/// One row of the MPEG-DASH manifest table.
struct ManifestRow {
    case: &'static str,
    extra: serde_json::Value,
    /// Whether every `S` element carries the fragment's own number.
    fragment_numbers: bool,
    /// Whether every `S` element carries its accurate timestamp.
    timestamps: bool,
    /// The presentation the manifest declares.
    presentation: &'static str,
}

/// RULE: the MPEG-DASH manifest of a session states the parameters that session
/// was minted with — "if DisplayFragmentNumber is set to ALWAYS, the Kinesis Video
/// Streams fragment number is added to each S element in the manifest file with
/// the attribute name kvs:fn", and the same for the accurate timestamp under
/// `kvs:ts`.
///
/// The rows differ along what the rule generalises over: both values of each of
/// the two members, their combination, and a complete presentation beside one a
/// player re-reads.
#[tokio::test]
async fn a_dash_session_manifest_states_the_parameters_it_was_minted_with() {
    let rows: Vec<ManifestRow> = vec![
        ManifestRow {
            case: "the defaults",
            extra: serde_json::json!({}),
            fragment_numbers: false,
            timestamps: false,
            presentation: "static",
        },
        ManifestRow {
            case: "fragment numbers on every S element",
            extra: serde_json::json!({ "DisplayFragmentNumber": "ALWAYS" }),
            fragment_numbers: true,
            timestamps: false,
            presentation: "static",
        },
        ManifestRow {
            case: "accurate timestamps on every S element",
            extra: serde_json::json!({ "DisplayFragmentTimestamp": "ALWAYS" }),
            fragment_numbers: false,
            timestamps: true,
            presentation: "static",
        },
        ManifestRow {
            case: "both on every S element",
            extra: serde_json::json!({
                "DisplayFragmentNumber": "ALWAYS",
                "DisplayFragmentTimestamp": "ALWAYS",
            }),
            fragment_numbers: true,
            timestamps: true,
            presentation: "static",
        },
    ];

    let harness = seeded();
    for row in rows {
        let case = row.case;
        let url = minted_session_url(
            &harness,
            case,
            "/getDASHStreamingSessionURL",
            "DASHStreamingSessionURL",
            &on_demand_body(&row.extra, "DASHFragmentSelector"),
        )
        .await;
        let manifest = text_body(&harness.fetch(&url).await);

        let elements: Vec<&str> = manifest
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("<S "))
            .collect();
        assert_eq!(
            elements.len(),
            SEEDED_FRAGMENTS as usize,
            "case {case}: expected one S element per fragment, got {elements:?}"
        );
        for (index, element) in elements.iter().enumerate() {
            let number = fragment_number(index as u64);
            let carries_number = element.contains(&format!("kvs:fn=\"{number}\""));
            assert_eq!(
                carries_number, row.fragment_numbers,
                "case {case}: expected kvs:fn on {element:?}: {}",
                row.fragment_numbers
            );
            let carries_timestamp =
                element.contains(&format!("kvs:ts=\"{}\"", reported_producer_instant(index as u64)));
            assert_eq!(
                carries_timestamp, row.timestamps,
                "case {case}: expected kvs:ts on {element:?}: {}",
                row.timestamps
            );
        }
        assert!(
            manifest.contains(&format!("type=\"{}\"", row.presentation)),
            "case {case}: expected a {:?} presentation, got {manifest:?}",
            row.presentation
        );
    }
}

/// One row of the growing-playlist table: a session to mint, the stream a
/// fragment is ingested into after it was minted, and whether that fragment
/// belongs in the next document the session serves.
struct GrowthRow {
    case: &'static str,
    /// The operation that mints the session, and the output member its URL is
    /// reported in.
    path: &'static str,
    member: &'static str,
    /// The minting request.
    body: serde_json::Value,
    /// The stream the session plays, which is also the one appended to.
    stream: &'static str,
    /// Whether this session's playlist GROWS: whether the fragment ingested
    /// after the mint must appear in it, and whether the document must leave
    /// itself open instead of declaring itself complete.
    grows: bool,
}

/// The document that LISTS a session's fragments: the MEDIA playlist the HLS
/// master playlist names, or the MPEG-DASH manifest, which is itself the root.
async fn listing_document(harness: &Harness, case: &str, url: &str) -> String {
    let root = harness.fetch(url).await;
    assert_eq!(
        root.status().as_u16(),
        200,
        "case {case}: the session's root resource must be served, got {} {:?}",
        root.status(),
        text_body(&root)
    );
    let document = text_body(&root);
    if !url.contains("/hls/") {
        return document;
    }
    let variant = referenced_resources(&document)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case}: the master playlist must name a media playlist"));
    let media = harness.fetch(&sibling_url(url, &variant)).await;
    assert_eq!(
        media.status().as_u16(),
        200,
        "case {case}: the media playlist must be served, got {} {:?}",
        media.status(),
        text_body(&media)
    );
    text_body(&media)
}

/// The fragment numbers a playlist or a manifest lists, in the order it lists
/// them: every resource it names for media addresses its fragment by number.
fn listed_fragment_numbers(document: &str) -> Vec<String> {
    referenced_resources(document)
        .iter()
        .filter_map(|resource| {
            let field = format!("{}=", archived_media::state::FRAGMENT_NUMBER_FIELD);
            resource
                .split_once(&field)
                .map(|(_, rest)| rest.split('&').next().unwrap_or_default().to_string())
        })
        .collect()
}

/// The `EXT-X-MEDIA-SEQUENCE` an HLS media playlist declares, or `None` for a
/// document that is no HLS playlist.
fn media_sequence(document: &str) -> Option<u64> {
    document
        .lines()
        .filter_map(|line| line.trim().strip_prefix("#EXT-X-MEDIA-SEQUENCE:"))
        .find_map(|value| value.parse().ok())
}

/// Ingest ONE more fragment into `stream`'s archive, the way the ingest plane
/// appends to it, and answer its fragment number.
///
/// A fragment of the live stream is stamped now, as one just ingested is; a
/// fragment of the seeded archive continues that archive's own timestamps, so it
/// falls inside the range the seeded sessions ask for.
fn ingest_another_fragment(harness: &Harness, stream: &str) -> String {
    let index = match stream == LIVE_STREAM {
        true => LIVE_FRAGMENTS,
        false => SEEDED_FRAGMENTS,
    };
    let (producer, server) = match stream == LIVE_STREAM {
        true => {
            let now = SystemTime::now();
            (now, now)
        }
        false => (
            from_epoch_millis(producer_secs(index) * 1000),
            from_epoch_millis(server_secs(index) * 1000),
        ),
    };
    let state = service_state(&*harness.engine).expect("the service store");
    let mut state = state.lock();
    assert!(
        state.append_fragment(
            stream,
            StoredFragment::new(
                fragment_number(index),
                producer,
                server,
                1000,
                media(index),
            ),
        ),
        "case ingest into {stream}: the stream must exist"
    );
    fragment_number(index)
}

/// RULE: a streaming session whose playback mode defines a GROWING playlist —
/// the two live modes, whose playlist "is continually updated with the latest
/// fragments as they become available" and which "will continue to include newly
/// ingested fragments until the session expires" — re-applies its own selection
/// (window, timestamp source and fragment cap) to the archive at every fetch, so
/// a fragment ingested after the session was minted is listed; a session whose
/// mode plays a closed stretch of the past keeps listing what it was minted
/// with, and is the only one that declares itself complete.
///
/// The rows differ along what the rule generalises over: each playback mode, a
/// range with and without an `EndTimestamp`, and both playback protocols — the
/// HLS media playlist and the MPEG-DASH manifest.
#[tokio::test]
async fn a_growing_session_playlist_lists_fragments_ingested_after_the_mint() {
    let rows: Vec<GrowthRow> = vec![
        GrowthRow {
            case: "LIVE over HLS",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: serde_json::json!({ "StreamName": LIVE_STREAM, "PlaybackMode": "LIVE" }),
            stream: LIVE_STREAM,
            grows: true,
        },
        GrowthRow {
            case: "LIVE over MPEG-DASH",
            path: "/getDASHStreamingSessionURL",
            member: "DASHStreamingSessionURL",
            body: serde_json::json!({ "StreamName": LIVE_STREAM, "PlaybackMode": "LIVE" }),
            stream: LIVE_STREAM,
            grows: true,
        },
        GrowthRow {
            case: "LIVE_REPLAY with no EndTimestamp over HLS",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "MaxMediaPlaylistFragmentResults": 10,
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            stream: STREAM,
            grows: true,
        },
        GrowthRow {
            case: "LIVE_REPLAY with no EndTimestamp over MPEG-DASH",
            path: "/getDASHStreamingSessionURL",
            member: "DASHStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "MaxManifestFragmentResults": 10,
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            stream: STREAM,
            grows: true,
        },
        GrowthRow {
            case: "LIVE_REPLAY with an EndTimestamp the ingest falls inside, over HLS",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "MaxMediaPlaylistFragmentResults": 10,
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
            stream: STREAM,
            grows: true,
        },
        GrowthRow {
            case: "ON_DEMAND over HLS",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: on_demand_body(
                &serde_json::json!({ "MaxMediaPlaylistFragmentResults": 10 }),
                "HLSFragmentSelector",
            ),
            stream: STREAM,
            grows: false,
        },
        GrowthRow {
            case: "ON_DEMAND over MPEG-DASH",
            path: "/getDASHStreamingSessionURL",
            member: "DASHStreamingSessionURL",
            body: on_demand_body(
                &serde_json::json!({ "MaxManifestFragmentResults": 10 }),
                "DASHFragmentSelector",
            ),
            stream: STREAM,
            grows: false,
        },
    ];

    for row in rows {
        let case = row.case;
        // Each row ingests into its own emulator, so one row's appended
        // fragment cannot be what the next row observes.
        let harness = seeded();
        let url = minted_session_url(&harness, case, row.path, row.member, &row.body).await;

        let before = listing_document(&harness, case, &url).await;
        let listed_before = listed_fragment_numbers(&before);
        assert!(
            !listed_before.is_empty(),
            "case {case}: the minted session must list the fragments it serves, got {before:?}"
        );

        // A document that tells a player to come back for more must not also
        // declare itself finished, and the complete one must.
        let declares_complete = before.contains("#EXT-X-ENDLIST")
            || before.contains("#EXT-X-PLAYLIST-TYPE:VOD")
            || before.contains("type=\"static\"");
        assert_eq!(
            declares_complete, !row.grows,
            "case {case}: a document declares itself complete exactly when re-reading it \
             cannot show a new fragment, got {before:?}"
        );

        let ingested = ingest_another_fragment(&harness, row.stream);
        let after = listing_document(&harness, case, &url).await;
        let listed_after = listed_fragment_numbers(&after);
        assert_eq!(
            listed_after.contains(&ingested), row.grows,
            "case {case}: expected the fragment {ingested} ingested after the mint to be \
             listed: {}, got {listed_after:?}",
            row.grows
        );
        if !row.grows {
            assert_eq!(
                listed_after, listed_before,
                "case {case}: a session over a closed range keeps the playlist it was minted \
                 with, got {listed_after:?}"
            );
            continue;
        }

        // The media of the newly listed fragment is served too, so what grew is
        // the session rather than the text of its playlist.
        let segment = referenced_resources(&after)
            .into_iter()
            .find(|resource| resource.contains(&ingested))
            .unwrap_or_else(|| panic!("case {case}: the document must name the new fragment"));
        let served = harness.fetch(&sibling_url(&url, &segment)).await;
        assert_eq!(
            served.status().as_u16(),
            200,
            "case {case}: the media of the newly listed fragment must be served, got {} {:?}",
            served.status(),
            text_body(&served)
        );
        assert!(
            !served.body().is_empty(),
            "case {case}: the media of the newly listed fragment must carry bytes"
        );

        // And a playlist that slides forward renumbers: the media sequence of
        // its first segment rises by however many fragments left the front, so
        // no number names two fragments.
        if let (Some(sequence_before), Some(sequence_after)) =
            (media_sequence(&before), media_sequence(&after))
        {
            let slid = listed_before
                .iter()
                .filter(|number| !listed_after.contains(number))
                .count();
            assert_eq!(
                sequence_after.saturating_sub(sequence_before),
                slid as u64,
                "case {case}: {slid} fragments left the front of {listed_before:?} -> \
                 {listed_after:?}, but the media sequence went {sequence_before} -> \
                 {sequence_after}"
            );
        }
    }
}

/// The fragment numbers of the seeded fragments at `indices`, in that order.
fn fragment_numbers(indices: &[u64]) -> Vec<String> {
    indices.iter().copied().map(fragment_number).collect()
}

/// The stream the anchor rows select from: an archive whose fragments each play
/// for [`SLOW_FRAGMENT_MILLIS`], so no replay can advance while the test runs.
const ANCHOR_STREAM: &str = "anchor-stream";

/// How many fragments [`ANCHOR_STREAM`] holds: one more than the live modes'
/// documented default maximum, so a session over its whole range has a
/// fragment the maximum leaves out.
const ANCHOR_FRAGMENTS: u64 = 6;

/// How long one fragment of [`ANCHOR_STREAM`] plays for: an hour, orders of
/// magnitude longer than this suite can take, so the pace a replay advances at
/// cannot move its playlist and the mint's own selection is what every
/// assertion about that stream reads.
const SLOW_FRAGMENT_MILLIS: u64 = 60 * 60 * 1000;

/// Seed `stream` into an already-built harness, with `count` fragments spaced
/// exactly as [`seeded`]'s own archive is but each playing for
/// `duration_millis`, so a case can choose the pace its playback advances at.
fn seed_stream_with_durations(
    harness: &Harness,
    stream: &str,
    count: u64,
    duration_millis: u64,
) {
    let archived = ArchivedStream::new(stream, REGION, from_epoch_millis(T0_SECS * 1000), 24);
    register_stream(&harness.engine, &archived);
    let state = service_state(&*harness.engine).expect("the service store");
    let mut state = state.lock();
    state.put_stream(archived);
    for index in 0..count {
        assert!(
            state.append_fragment(
                stream,
                StoredFragment::new(
                    fragment_number(index),
                    from_epoch_millis(producer_secs(index) * 1000),
                    from_epoch_millis(server_secs(index) * 1000),
                    duration_millis,
                    media(index),
                ),
            ),
            "case seed {stream} fragment {index}: the stream must exist"
        );
    }
}

/// One row of the playlist-anchor table: a session over an archive that cannot
/// advance while the test runs, and the fragments its document lists.
struct AnchorRow {
    case: &'static str,
    /// The operation that mints the session, and the output member its URL is
    /// reported in.
    path: &'static str,
    member: &'static str,
    /// The minting request.
    body: serde_json::Value,
    /// The seeded fragments the document must list, by index.
    listed: &'static [u64],
    /// The `EXT-X-MEDIA-SEQUENCE` an HLS media playlist must declare; `None`
    /// for an MPEG-DASH manifest, which declares no such tag.
    sequence: Option<u64>,
}

/// RULE: which fragments of its window a session's maximum is filled from is
/// the playback mode's — a range mode's playlist is ANCHORED at the start of
/// the requested range ("it starts by including fragments from a given start
/// time" for LIVE_REPLAY, "the oldest fragments are returned, up to this
/// maximum number" for ON_DEMAND), so its first segment's media sequence number
/// is 0 and the fragments the maximum leaves out are the LATER ones, while LIVE
/// is filled from the stream head ("the most recent fragments are returned up to
/// this value"), which puts the ones it leaves out in front of the playlist.
///
/// The rows differ along what the rule generalises over: the playback mode,
/// where the maximum comes from (the mode's documented default or the request's
/// own, at several values), a range left open and one carrying an end, and both
/// playback protocols — the HLS media playlist and the MPEG-DASH manifest.
#[tokio::test]
async fn a_sessions_fragment_maximum_is_filled_from_the_end_its_mode_starts_at() {
    let rows: Vec<AnchorRow> = vec![
        AnchorRow {
            // The documented default for the live modes is 5 fragments, one
            // fewer than this archive holds.
            case: "LIVE_REPLAY at its default maximum, over HLS",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": ANCHOR_STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            listed: &[0, 1, 2, 3, 4],
            sequence: Some(0),
        },
        AnchorRow {
            case: "LIVE_REPLAY at its default maximum, over MPEG-DASH",
            path: "/getDASHStreamingSessionURL",
            member: "DASHStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": ANCHOR_STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            listed: &[0, 1, 2, 3, 4],
            sequence: None,
        },
        AnchorRow {
            case: "LIVE_REPLAY at a requested maximum of 2, over HLS",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": ANCHOR_STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "MaxMediaPlaylistFragmentResults": 2,
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            listed: &[0, 1],
            sequence: Some(0),
        },
        AnchorRow {
            // A range that carries an end holds the four fragments stamped at
            // or before it, of which the maximum lists the first three.
            case: "LIVE_REPLAY at a requested maximum of 3, over a range with an end, over \
                   MPEG-DASH",
            path: "/getDASHStreamingSessionURL",
            member: "DASHStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": ANCHOR_STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "MaxManifestFragmentResults": 3,
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 35,
                    },
                },
            }),
            listed: &[0, 1, 2],
            sequence: None,
        },
        AnchorRow {
            // The static mode, anchored at the same end of the same range.
            case: "ON_DEMAND at a requested maximum of 2, over HLS",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": ANCHOR_STREAM,
                "PlaybackMode": "ON_DEMAND",
                "MaxMediaPlaylistFragmentResults": 2,
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 1000,
                    },
                },
            }),
            listed: &[0, 1],
            sequence: Some(0),
        },
        AnchorRow {
            // The mode filled from the OTHER end: the live stream holds seven
            // fragments, two more than the default maximum, so the two oldest
            // are the ones left out and the sequence starts past them.
            case: "LIVE at its default maximum, over HLS",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: serde_json::json!({ "StreamName": LIVE_STREAM, "PlaybackMode": "LIVE" }),
            listed: &[2, 3, 4, 5, 6],
            sequence: Some(2),
        },
        AnchorRow {
            case: "LIVE at a requested maximum of 2, over MPEG-DASH",
            path: "/getDASHStreamingSessionURL",
            member: "DASHStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": LIVE_STREAM,
                "PlaybackMode": "LIVE",
                "MaxManifestFragmentResults": 2,
            }),
            listed: &[5, 6],
            sequence: None,
        },
    ];

    for row in rows {
        let case = row.case;
        let harness = seeded();
        seed_stream_with_durations(
            &harness,
            ANCHOR_STREAM,
            ANCHOR_FRAGMENTS,
            SLOW_FRAGMENT_MILLIS,
        );
        let url = minted_session_url(&harness, case, row.path, row.member, &row.body).await;

        let document = listing_document(&harness, case, &url).await;
        let listed = listed_fragment_numbers(&document);
        assert_eq!(
            listed,
            fragment_numbers(row.listed),
            "case {case}: expected the session to list {:?}, got {listed:?} from {document:?}",
            fragment_numbers(row.listed)
        );
        if let Some(sequence) = row.sequence {
            let declared = media_sequence(&document);
            assert_eq!(
                declared,
                Some(sequence),
                "case {case}: expected the playlist to declare media sequence {sequence}, got \
                 {declared:?} from {document:?}"
            );
        }
    }
}

/// How long a row of the advance table waits for the playlist to reach the
/// selection the clock owes it: far longer than the fragment durations it
/// paces at need, so a loaded host cannot fail it.
const ADVANCE_DEADLINE: Duration = Duration::from_secs(120);

/// How long a row waits between two looks at the playlist.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// One row of the playlist-advance table: a LIVE_REPLAY session whose window
/// holds more fragments than its maximum, and the selection its document
/// settles on once the fragments in front of it have played.
struct AdvanceRow {
    case: &'static str,
    /// The operation that mints the session, and the output member its URL is
    /// reported in.
    path: &'static str,
    member: &'static str,
    /// The minting request.
    body: serde_json::Value,
    /// The fragments the document lists at the MINT, stated only where the
    /// window cannot have advanced yet — a window that holds no more fragments
    /// than the maximum — so the assertion cannot race the pace.
    minted: Option<&'static [u64]>,
    /// Whether one more fragment is ingested into the window after the mint,
    /// which is what gives a window at its maximum somewhere to advance to.
    ingest: bool,
    /// The fragments the document settles on, by index: the window can advance
    /// no further than the newest fragment the archive holds.
    settled: &'static [u64],
    /// The `EXT-X-MEDIA-SEQUENCE` the HLS media playlist then declares; `None`
    /// for an MPEG-DASH manifest, which declares no such tag.
    sequence: Option<u64>,
}

/// RULE: a LIVE_REPLAY playlist ADVANCES over its range at the pace the
/// fragments it lists play at — "fragments are added as the duration of the
/// next fragment elapses. For example, if the fragments in the session are two
/// seconds long, then a new fragment is added to the media playlist every two
/// seconds" — so each fragment duration that goes by drops the fragment at the
/// front of the playlist, raises the media sequence number of its first segment
/// by one and takes in the next fragment of the range, a fragment ingested
/// after the mint included; it advances no further than the newest fragment the
/// archive holds, so the playlist keeps the size its maximum asks for.
///
/// The rows differ along what the rule generalises over: where the maximum
/// comes from (the mode's documented default or the request's own), whether the
/// fragment the playlist advances onto was ingested before or after the mint, a
/// range left open and one carrying an end, how many durations have to elapse,
/// and both playback protocols.
///
/// Each row waits for the CONDITION — the selection the clock owes the session
/// — rather than for a span, under a deadline far longer than the archive's own
/// fragment durations.
#[tokio::test]
async fn a_live_replay_playlist_advances_as_the_fragments_it_lists_play_out() {
    let rows: Vec<AdvanceRow> = vec![
        AdvanceRow {
            // The documented default of five, which the seeded range holds
            // exactly: nothing can leave the playlist until a sixth fragment
            // is ingested, and then exactly one does.
            case: "the default maximum, with a fragment ingested after the mint, over HLS",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            minted: Some(&[0, 1, 2, 3, 4]),
            ingest: true,
            settled: &[1, 2, 3, 4, 5],
            sequence: Some(1),
        },
        AdvanceRow {
            case: "the default maximum, with a fragment ingested after the mint, over MPEG-DASH",
            path: "/getDASHStreamingSessionURL",
            member: "DASHStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            minted: Some(&[0, 1, 2, 3, 4]),
            ingest: true,
            settled: &[1, 2, 3, 4, 5],
            sequence: None,
        },
        AdvanceRow {
            // A maximum the archived range already exceeds by one: the playlist
            // advances one fragment on the clock alone, nothing ingested.
            case: "a requested maximum of 4 over a range of 5, over HLS",
            path: "/getHLSStreamingSessionURL",
            member: "HLSStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "MaxMediaPlaylistFragmentResults": 4,
                "HLSFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": { "StartTimestamp": T0_SECS },
                },
            }),
            minted: None,
            ingest: false,
            settled: &[1, 2, 3, 4],
            sequence: Some(1),
        },
        AdvanceRow {
            // Two durations' worth of advance, over a range that carries an
            // end: it holds the four fragments stamped at or before it.
            case: "a requested maximum of 2 over a range with an end, over MPEG-DASH",
            path: "/getDASHStreamingSessionURL",
            member: "DASHStreamingSessionURL",
            body: serde_json::json!({
                "StreamName": STREAM,
                "PlaybackMode": "LIVE_REPLAY",
                "MaxManifestFragmentResults": 2,
                "DASHFragmentSelector": {
                    "FragmentSelectorType": "PRODUCER_TIMESTAMP",
                    "TimestampRange": {
                        "StartTimestamp": T0_SECS,
                        "EndTimestamp": T0_SECS + 35,
                    },
                },
            }),
            minted: None,
            ingest: false,
            settled: &[2, 3],
            sequence: None,
        },
    ];

    for row in rows {
        let case = row.case;
        // Each row runs on its own emulator, so one row's appended fragment
        // cannot be what the next row observes.
        let harness = seeded();
        let url = minted_session_url(&harness, case, row.path, row.member, &row.body).await;

        if let Some(minted) = row.minted {
            let document = listing_document(&harness, case, &url).await;
            let listed = listed_fragment_numbers(&document);
            assert_eq!(
                listed,
                fragment_numbers(minted),
                "case {case}: expected the minted session to list {:?}, got {listed:?} from \
                 {document:?}",
                fragment_numbers(minted)
            );
        }
        let settled = fragment_numbers(row.settled);
        if row.ingest {
            // The fragment the playlist must advance onto, appended the way the
            // ingest plane appends to the archive.
            let ingested = ingest_another_fragment(&harness, STREAM);
            assert!(
                settled.contains(&ingested),
                "case {case}: expected the settled playlist {settled:?} to be the one carrying \
                 the ingested fragment {ingested}"
            );
        }

        // The selection the clock owes this session, waited for by looking
        // again rather than by sleeping a span.
        let document = poll_until_listed(&harness, case, &url, &settled).await;
        if let Some(sequence) = row.sequence {
            let declared = media_sequence(&document);
            assert_eq!(
                declared,
                Some(sequence),
                "case {case}: expected the advanced playlist to declare media sequence \
                 {sequence}, got {declared:?} from {document:?}"
            );
        }

        // The media of the fragment the playlist advanced ONTO is served too,
        // so what moved is the session rather than the text of its document.
        let newest = settled
            .last()
            .unwrap_or_else(|| panic!("case {case}: the settled playlist names a fragment"));
        let segment = referenced_resources(&document)
            .into_iter()
            .find(|resource| resource.contains(newest))
            .unwrap_or_else(|| panic!("case {case}: the document must name {newest}"));
        let served = harness.fetch(&sibling_url(&url, &segment)).await;
        assert_eq!(
            served.status().as_u16(),
            200,
            "case {case}: the media of {newest} must be served, got {} {:?}",
            served.status(),
            text_body(&served)
        );
        assert!(
            !served.body().is_empty(),
            "case {case}: the media of {newest} must carry bytes"
        );
    }
}

/// Look at the session's listing document again until it lists exactly
/// `expected`, and answer that document.
///
/// Waits for the CONDITION, not for a span: a LIVE_REPLAY playlist advances as
/// the fragments it lists play out, so the wait is bounded by
/// [`ADVANCE_DEADLINE`], far longer than those durations need.
async fn poll_until_listed(
    harness: &Harness,
    case: &str,
    url: &str,
    expected: &[String],
) -> String {
    let deadline = std::time::Instant::now() + ADVANCE_DEADLINE;
    let mut document = String::new();
    while std::time::Instant::now() < deadline {
        document = listing_document(harness, case, url).await;
        if listed_fragment_numbers(&document) == expected {
            return document;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!(
        "case {case}: expected the playlist to list {expected:?} before the deadline, got {:?} \
         from {document:?}",
        listed_fragment_numbers(&document)
    );
}

/// RULE: a session's media fragment carries the frames the archive holds for that
/// fragment and nothing else, in the container the session's `ContainerFormat`
/// asks for; its initialization carries the track's own declaration and no media.
#[tokio::test]
async fn a_session_media_fragment_carries_the_frames_the_archive_holds() {
    let coded = coded_video();
    let harness = seeded();

    // The MP4 pair: the initialization declares the archived track and carries no
    // sample; each media fragment carries exactly that fragment's access units.
    let url = minted_session_url(
        &harness,
        "MP4 fragments",
        "/getHLSStreamingSessionURL",
        "HLSStreamingSessionURL",
        &on_demand_body(&serde_json::json!({}), "HLSFragmentSelector"),
    )
    .await;
    let variant = referenced_resources(&text_body(&harness.fetch(&url).await))
        .into_iter()
        .next()
        .expect("the master playlist names a media playlist");
    let playlist = text_body(&harness.fetch(&sibling_url(&url, &variant)).await);

    let init_uri = playlist
        .lines()
        .find_map(|line| line.trim().strip_prefix("#EXT-X-MAP:URI=\""))
        .and_then(|rest| rest.split('"').next())
        .expect("the media playlist names an initialization fragment")
        .to_string();
    let init = harness.fetch(&sibling_url(&url, &init_uri)).await;
    let init_bytes = init.body().to_vec();
    assert_is_mp4("the initialization fragment", &init_bytes);
    let declared = clip_track("the initialization fragment", &init_bytes);
    assert!(
        declared.samples.is_empty(),
        "case the initialization fragment: it carries only the codec private data, got {} \
         samples",
        declared.samples.len()
    );
    assert_eq!(
        (declared.width, declared.height),
        (
            u16::try_from(CODED_WIDTH).expect("a short side"),
            u16::try_from(CODED_HEIGHT).expect("a short side")
        ),
        "case the initialization fragment: it declares the archived track's geometry"
    );
    assert_eq!(
        (declared.seq_param_set, declared.pic_param_set),
        (coded.seq_param_set.clone(), coded.pic_param_set.clone()),
        "case the initialization fragment: it carries the archived track's parameter sets"
    );

    let segments: Vec<String> = playlist
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();
    for (index, segment) in segments.iter().enumerate() {
        let case = format!("the MP4 media fragment of seeded fragment {index}");
        assert!(
            segment.contains(&format!("FragmentNumber={}", fragment_number(index as u64))),
            "case {case}: the segment must address that fragment, got {segment:?}"
        );
        let served = harness.fetch(&sibling_url(&url, segment)).await;
        let bytes = served.body().to_vec();
        assert_is_mp4(&case, &bytes);
        let track = clip_track(&case, &bytes);
        assert_eq!(
            track.samples,
            media_units(index as u64),
            "case {case}: expected the archived fragment's access units"
        );
    }

    // The MPEG TS fragment: whole transport packets of the same frames.
    let ts_url = minted_session_url(
        &harness,
        "MPEG TS fragments",
        "/getHLSStreamingSessionURL",
        "HLSStreamingSessionURL",
        &on_demand_body(
            &serde_json::json!({ "ContainerFormat": "MPEG_TS" }),
            "HLSFragmentSelector",
        ),
    )
    .await;
    let ts_variant = referenced_resources(&text_body(&harness.fetch(&ts_url).await))
        .into_iter()
        .next()
        .expect("the master playlist names a media playlist");
    let ts_playlist = text_body(&harness.fetch(&sibling_url(&ts_url, &ts_variant)).await);
    let ts_segments: Vec<String> = ts_playlist
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();
    assert_eq!(
        ts_segments.len(),
        SEEDED_FRAGMENTS as usize,
        "case the MPEG TS playlist: expected one segment per fragment, got {ts_segments:?}"
    );
    for (index, segment) in ts_segments.iter().enumerate() {
        let case = format!("the MPEG TS fragment of seeded fragment {index}");
        let served = harness.fetch(&sibling_url(&ts_url, segment)).await;
        let bytes = served.body().to_vec();
        assert_eq!(
            served_type(&served),
            TS_FRAGMENT_TYPE,
            "case {case}: an MPEG TS fragment is served as one"
        );
        assert!(
            !bytes.is_empty() && bytes.len() % TS_PACKET_BYTES == 0,
            "case {case}: a transport stream is whole 188-byte packets, got {} bytes",
            bytes.len()
        );
        assert!(
            bytes
                .chunks(TS_PACKET_BYTES)
                .all(|packet| packet.first() == Some(&0x47)),
            "case {case}: every transport packet starts with the sync byte"
        );
    }
}

/// One row of the refusal table: a playback address, and why nothing is served at
/// it.
struct PlaybackRefusalRow {
    case: &'static str,
    method: &'static str,
    /// The path and query, with `{token}` standing for the live session's token.
    target: &'static str,
    status: u16,
}

/// RULE: a playback address serves the session its token names, while that
/// session's own `Expires` window is open, and nothing otherwise — "when a session
/// expires, no new calls to GetHLSMasterPlaylist, GetHLSMediaPlaylist,
/// GetMP4InitFragment, GetMP4MediaFragment, or GetTSFragment can be made for that
/// session".
///
/// The rows differ along what the rule generalises over: a token no session
/// answers to, a token whose session has expired, a token for the other playback
/// protocol, no token at all, a resource of the other protocol's plane, a track
/// and a fragment the session does not serve, and a method a player does not read
/// with.
#[tokio::test]
async fn a_playback_address_serves_nothing_it_cannot_resolve() {
    let rows: &[PlaybackRefusalRow] = &[
        PlaybackRefusalRow {
            case: "a token no session answers to",
            method: "GET",
            target: "/hls/v1/getHLSMasterPlaylist.m3u8?SessionToken=00000000",
            status: 404,
        },
        PlaybackRefusalRow {
            case: "a session whose Expires window has closed",
            method: "GET",
            target: "/hls/v1/getHLSMasterPlaylist.m3u8?SessionToken=expired",
            status: 404,
        },
        PlaybackRefusalRow {
            case: "no token at all",
            method: "GET",
            target: "/hls/v1/getHLSMasterPlaylist.m3u8",
            status: 404,
        },
        PlaybackRefusalRow {
            case: "an HLS token on the MPEG-DASH manifest",
            method: "GET",
            target: "/dash/v1/getMPDManifest.mpd?SessionToken={token}",
            status: 404,
        },
        PlaybackRefusalRow {
            case: "the MPEG TS fragment of a session packaged as MP4",
            method: "GET",
            target: "/hls/v1/getTSFragment.ts?SessionToken={token}&FragmentNumber=9134385233318143200",
            status: 404,
        },
        PlaybackRefusalRow {
            case: "a track the session does not serve",
            method: "GET",
            target: "/hls/v1/getHLSMediaPlaylist.m3u8?SessionToken={token}&TrackNumber=9",
            status: 404,
        },
        PlaybackRefusalRow {
            case: "a fragment number the session does not serve",
            method: "GET",
            target: "/hls/v1/getMP4MediaFragment.mp4?SessionToken={token}&FragmentNumber=1",
            status: 404,
        },
        PlaybackRefusalRow {
            case: "a method a media player does not read with",
            method: "POST",
            target: "/hls/v1/getHLSMasterPlaylist.m3u8?SessionToken={token}",
            status: 405,
        },
    ];

    let harness = seeded();
    // A live session, whose token the rows address, and one whose window the clock
    // has already closed.
    let url = minted_session_url(
        &harness,
        "the live session",
        "/getHLSStreamingSessionURL",
        "HLSStreamingSessionURL",
        &on_demand_body(&serde_json::json!({}), "HLSFragmentSelector"),
    )
    .await;
    let token = url
        .rsplit_once("SessionToken=")
        .map(|(_, token)| token.to_string())
        .expect("the session URL carries a token");
    {
        use archived_media::state::{
            SessionProtocol, SessionWindow, StreamingSession, TimestampOrigin,
        };

        let state = service_state(&*harness.engine).expect("the service store");
        let mut state = state.lock();
        let created_at = SystemTime::now() - Duration::from_secs(3600);
        state.record_session(StreamingSession {
            token: "expired".to_string(),
            stream_name: STREAM.to_string(),
            protocol: SessionProtocol::Hls,
            playback_mode: "ON_DEMAND".to_string(),
            origin: TimestampOrigin::Producer,
            container_format: "FRAGMENTED_MP4".to_string(),
            discontinuity_mode: Some("NEVER".to_string()),
            display_fragment_timestamp: "NEVER".to_string(),
            display_fragment_number: None,
            fragment_numbers: vec![fragment_number(0)],
            window: SessionWindow::Range {
                start_millis: T0_SECS * 1000,
                end_millis: (T0_SECS + 1000) * 1000,
            },
            max_fragments: 1000,
            created_at,
            expires_in_secs: 300,
        });
    }

    for row in rows {
        let target = row.target.replace("{token}", &token);
        let response = harness
            .fetch_with(row.method, &format!("https://{EMULATOR_ENDPOINT}{target}"))
            .await;
        assert_eq!(
            response.status().as_u16(),
            row.status,
            "case {}: expected HTTP {} for {} {target}, got {} {:?}",
            row.case,
            row.status,
            row.method,
            response.status(),
            text_body(&response)
        );
    }

    // And the live session is served, so the rows above fail for the reason each
    // names rather than because nothing is served at all.
    let served = harness.fetch(&url).await;
    assert_eq!(
        served.status().as_u16(),
        200,
        "case the live session: its master playlist must be served, got {}",
        served.status()
    );
}

// ---------------------------------------------------------------------------
// The documented span, along the axes the suite above holds fixed.
// ---------------------------------------------------------------------------
//
// [`MAX_RANGE_SPAN_SECS`], [`RANGE_TOO_WIDE`] and [`RANGE_OUT_OF_ORDER`] above
// state the bound and its two sentences; the two suites below vary what that one
// holds fixed — where the range sits on the time line, which member addresses the
// stream, which timestamp field is selected on — and what an admitted range is
// actually answered with.

/// The code a request refused on its arguments alone carries.
const INVALID_ARGUMENT: &str = "InvalidArgumentException";

/// The code a range the addressed archive holds no fragment in carries.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// An operation reading a timestamp range whose `EndTimestamp` documents a
/// maximum span, with whatever a request to it needs beside that range, as the
/// two suites below address it.
#[derive(Clone, Copy)]
struct AddressedRange {
    /// The path a request is posted to.
    path: &'static str,
    /// The member a request carries its fragment selector in.
    selector_member: &'static str,
    /// The `PlaybackMode` a streaming-session request has to ask for to carry a
    /// range at all; [`None`] for the clip, which has no playback mode.
    playback_mode: Option<&'static str>,
}

/// `GetClip`, which reads its range from `ClipFragmentSelector`.
const CLIP: AddressedRange = AddressedRange {
    path: "/getClip",
    selector_member: "ClipFragmentSelector",
    playback_mode: None,
};

/// `GetHLSStreamingSessionURL` in `mode`, which reads its range from
/// `HLSFragmentSelector`.
const fn hls(mode: &'static str) -> AddressedRange {
    AddressedRange {
        path: "/getHLSStreamingSessionURL",
        selector_member: "HLSFragmentSelector",
        playback_mode: Some(mode),
    }
}

/// `GetDASHStreamingSessionURL` in `mode`, which reads its range from
/// `DASHFragmentSelector`.
const fn dash(mode: &'static str) -> AddressedRange {
    AddressedRange {
        path: "/getDASHStreamingSessionURL",
        selector_member: "DASHFragmentSelector",
        playback_mode: Some(mode),
    }
}

/// A request to `operation` addressing `stream` (the stream-naming members),
/// selecting on `selector_type` over the range `[start, end]` given as offsets
/// in seconds from [`T0_SECS`].
fn addressed_range_request(
    operation: AddressedRange,
    stream: &serde_json::Value,
    selector_type: &str,
    start: i64,
    end: i64,
) -> serde_json::Value {
    let mut extra = serde_json::json!({
        operation.selector_member: {
            "FragmentSelectorType": selector_type,
            "TimestampRange": {
                "StartTimestamp": T0_SECS + start,
                "EndTimestamp": T0_SECS + end,
            },
        },
    });
    if let Some(mode) = operation.playback_mode {
        extra["PlaybackMode"] = serde_json::json!(mode);
    }
    with_members(stream, &extra)
}

/// RULE: where a timestamp range's `EndTimestamp` documents how far past the
/// start it may sit, a range reaching further than that span is refused
/// `InvalidArgumentException` (400) with one fixed sentence naming that
/// documented span — whatever else the request varies — and the other clause of
/// the same documented sentence (the end must be later than the start) keeps its
/// own sentence.
///
/// The rows differ along what the rule generalises over and the suite above holds
/// fixed: where the range sits on the time line, which member addresses the
/// stream, which timestamp field is selected on, how far past the span the range
/// reaches, which operation reads it, which playback mode carries it, and which
/// of the sentence's two clauses is broken.
#[tokio::test]
async fn the_documented_span_refusal_holds_however_the_range_is_addressed_and_placed() {
    let harness = seeded();
    let by_name = serde_json::json!({ "StreamName": STREAM });
    let by_arn = serde_json::json!({ "StreamARN": harness.arn.clone() });
    // (case, operation, the stream-naming members, selector, start, end, sentence)
    let rows: Vec<(&str, AddressedRange, &serde_json::Value, &str, i64, i64, &str)> = vec![
        (
            "one second past the span, on a clip",
            CLIP,
            &by_name,
            "PRODUCER_TIMESTAMP",
            0,
            MAX_RANGE_SPAN_SECS + 1,
            RANGE_TOO_WIDE,
        ),
        (
            "thirty days, on a clip read on server timestamps",
            CLIP,
            &by_name,
            "SERVER_TIMESTAMP",
            0,
            30 * 86_400,
            RANGE_TOO_WIDE,
        ),
        (
            "a year, on a clip addressed by StreamARN",
            CLIP,
            &by_arn,
            "PRODUCER_TIMESTAMP",
            0,
            365 * 86_400,
            RANGE_TOO_WIDE,
        ),
        (
            "twice the span, on a clip starting a day before the archive",
            CLIP,
            &by_name,
            "PRODUCER_TIMESTAMP",
            -86_400,
            2 * MAX_RANGE_SPAN_SECS,
            RANGE_TOO_WIDE,
        ),
        (
            "a range that ends where it starts, on a clip",
            CLIP,
            &by_name,
            "PRODUCER_TIMESTAMP",
            0,
            0,
            RANGE_OUT_OF_ORDER,
        ),
        (
            "a range that runs backwards, on a clip",
            CLIP,
            &by_name,
            "PRODUCER_TIMESTAMP",
            40,
            10,
            RANGE_OUT_OF_ORDER,
        ),
        (
            "one second past the span, on an ON_DEMAND HLS session",
            hls("ON_DEMAND"),
            &by_name,
            "PRODUCER_TIMESTAMP",
            0,
            MAX_RANGE_SPAN_SECS + 1,
            RANGE_TOO_WIDE,
        ),
        (
            "thirty days, on a LIVE_REPLAY HLS session by StreamARN",
            hls("LIVE_REPLAY"),
            &by_arn,
            "SERVER_TIMESTAMP",
            0,
            30 * 86_400,
            RANGE_TOO_WIDE,
        ),
        (
            "a range that ends where it starts, on an ON_DEMAND HLS session",
            hls("ON_DEMAND"),
            &by_name,
            "PRODUCER_TIMESTAMP",
            0,
            0,
            RANGE_OUT_OF_ORDER,
        ),
        (
            "an hour past the span, on an ON_DEMAND MPEG-DASH session",
            dash("ON_DEMAND"),
            &by_name,
            "PRODUCER_TIMESTAMP",
            0,
            MAX_RANGE_SPAN_SECS + 3_600,
            RANGE_TOO_WIDE,
        ),
        (
            "a hundred days, on a LIVE_REPLAY MPEG-DASH session",
            dash("LIVE_REPLAY"),
            &by_name,
            "PRODUCER_TIMESTAMP",
            0,
            100 * 86_400,
            RANGE_TOO_WIDE,
        ),
    ];

    for (case, operation, stream, selector_type, start, end, expected) in rows {
        let body = addressed_range_request(operation, stream, selector_type, start, end);
        let response = harness.post(operation.path, &body).await;
        assert_eq!(
            response.status().as_u16(),
            400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        let reported = body_json(&response);
        let code = reported["__type"].as_str().unwrap_or_default();
        let message = reported["Message"]
            .as_str()
            .or_else(|| reported["message"].as_str())
            .unwrap_or_default();
        assert_eq!(
            (code, message),
            (INVALID_ARGUMENT, expected),
            "case {case:?}: expected ({INVALID_ARGUMENT:?}, {expected:?}), got \
             ({code:?}, {message:?})"
        );
        // The sentence is fixed: nothing the request carried — the stream it
        // addressed, either bound, the width it asked for — can be read out of it.
        for carried in [
            stream
                .as_object()
                .and_then(|members| members.values().next())
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            (T0_SECS + start).to_string(),
            (T0_SECS + end).to_string(),
            (end - start).to_string(),
        ] {
            assert!(
                !carried.is_empty() && !message.contains(&carried),
                "case {case:?}: the refusal must carry nothing of the request, but \
                 {message:?} contains {carried:?}"
            );
        }
    }
}

/// RULE: the documented span is INCLUSIVE — a range exactly that wide, or
/// narrower, is never refused for its span, and the request is then answered by
/// the addressed archive: served with the media it holds when the range selects a
/// fragment, and a missing resource when it holds none.
///
/// The rows differ along the width (the published bound, one second inside it,
/// the narrowest range there is), which operation reads it, which timestamp
/// field is selected on, and whether the archive answers the range at all.
#[tokio::test]
async fn a_range_on_or_inside_its_documented_span_is_answered_by_the_archive() {
    let harness = seeded();
    let by_name = serde_json::json!({ "StreamName": STREAM });
    // (case, operation, selector, start, end, the status expected)
    let rows: Vec<(&str, AddressedRange, &str, i64, i64, u16)> = vec![
        (
            "exactly the published span, on a clip over an archive that answers",
            CLIP,
            "PRODUCER_TIMESTAMP",
            0,
            MAX_RANGE_SPAN_SECS,
            200,
        ),
        (
            "one second inside the span, on a clip over an archive that answers",
            CLIP,
            "PRODUCER_TIMESTAMP",
            0,
            MAX_RANGE_SPAN_SECS - 1,
            200,
        ),
        (
            "exactly the published span, on a clip read on server timestamps",
            CLIP,
            "SERVER_TIMESTAMP",
            0,
            MAX_RANGE_SPAN_SECS,
            200,
        ),
        (
            "the narrowest range, on a clip between two fragments",
            CLIP,
            "PRODUCER_TIMESTAMP",
            1,
            2,
            404,
        ),
        (
            "exactly the published span, on a clip ending before the archive",
            CLIP,
            "PRODUCER_TIMESTAMP",
            -MAX_RANGE_SPAN_SECS - 1,
            -1,
            404,
        ),
        (
            "exactly the published span, on an ON_DEMAND HLS session",
            hls("ON_DEMAND"),
            "PRODUCER_TIMESTAMP",
            0,
            MAX_RANGE_SPAN_SECS,
            200,
        ),
        (
            "exactly the published span, on a LIVE_REPLAY HLS session on server timestamps",
            hls("LIVE_REPLAY"),
            "SERVER_TIMESTAMP",
            0,
            MAX_RANGE_SPAN_SECS,
            200,
        ),
        (
            "exactly the published span, on an ON_DEMAND MPEG-DASH session",
            dash("ON_DEMAND"),
            "PRODUCER_TIMESTAMP",
            0,
            MAX_RANGE_SPAN_SECS,
            200,
        ),
        (
            "exactly the published span, on an MPEG-DASH session before the archive",
            dash("ON_DEMAND"),
            "PRODUCER_TIMESTAMP",
            -MAX_RANGE_SPAN_SECS - 1,
            -1,
            404,
        ),
    ];

    for (case, operation, selector_type, start, end, status) in rows {
        let body = addressed_range_request(operation, &by_name, selector_type, start, end);
        let response = harness.post(operation.path, &body).await;
        assert_eq!(
            response.status().as_u16(),
            status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
        if status == 404 {
            let reported = body_json(&response);
            let code = reported["__type"].as_str().unwrap_or_default();
            assert_eq!(
                code, NOT_FOUND,
                "case {case:?}: expected {NOT_FOUND:?}, got {reported:?}"
            );
            continue;
        }
        if operation.path == CLIP.path {
            assert_eq!(
                content_type(&response),
                MP4_FRAGMENT_TYPE,
                "case {case:?}: GetClip downloads an MP4 clip"
            );
            assert_is_mp4(case, response.body());
            continue;
        }
        // A session call answers the URL a player fetches, which must lead back
        // to this emulator rather than to a real AWS host.
        let reported = body_json(&response);
        let url = reported
            .as_object()
            .and_then(|members| members.values().next())
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        assert!(
            url.starts_with(&format!("https://{EMULATOR_ENDPOINT}")),
            "case {case:?}: expected a session URL on this emulator, got {reported:?}"
        );
    }
}


