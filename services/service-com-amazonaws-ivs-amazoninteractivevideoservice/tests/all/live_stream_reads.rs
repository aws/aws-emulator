// SPDX-License-Identifier: CC0-1.0

//! `GetStream` end to end: what the emulator reports for the active (live) stream
//! on a channel.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws ivs get-stream` call arrives
//! — so what it asserts is the emulator's HTTP behaviour, not a handler called
//! directly.
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//! `GetStream` REPORTS the stream the store holds for a channel. It answers the
//! persisted stream while that stream is in the `LIVE` state, the operation's own
//! `ChannelNotBroadcasting` (404) for a channel that exists and holds no live
//! stream, and `ResourceNotFoundException` (404) for a channel ARN the store does
//! not hold at all; a request whose ARN is outside the modelled `ChannelArn`
//! constraint is refused with `ValidationException` (400) before any of that. Being
//! a read, repeating it leaves every one of those answers unchanged.
//!
//! Nothing in the IVS control plane starts a stream — a PUBLISHER does, on the contribute
//! address every channel announces — so the reading cases below bring a channel live
//! through `state::start_stream`, the one place a stream is minted and the same entry point
//! the contribute plane uses, reaching the store through the shared `Ivs` harness's engine
//! handle. The suites at the end of this file drive that plane itself, with a real
//! publisher over a real socket.

use std::collections::BTreeSet;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use service_com_amazonaws_ivs_amazoninteractivevideoservice::state;

// The one harness for this crate's suites, declared in `channel_lifecycle.rs` and
// shared by every suite here rather than copied: it owns the router, the request
// envelope, the reply shape and the store access the seeding below writes through.
use super::channel_lifecycle::{text, Ivs, Reply, CHANNEL_ARN_PREFIX};

/// The modelled upper bound on `ChannelArn` (`@length 1..128`).
const CHANNEL_ARN_MAX: usize = 128;

/// Call `GetStream` for `channel_arn`.
async fn get_stream(ivs: &Ivs, channel_arn: &str) -> Reply {
    ivs.call("GetStream", &json!({ "channelArn": channel_arn }))
        .await
}

/// Delete a channel, requiring the modelled 204.
async fn delete_channel(ivs: &Ivs, channel_arn: &str) {
    let reply = ivs.call("DeleteChannel", &json!({ "arn": channel_arn })).await;
    assert_eq!(
        reply.status, 204,
        "DeleteChannel {channel_arn}: expected HTTP 204, got {} {}",
        reply.status, reply.body
    );
}

/// Bring a channel live the way an ingest session does, returning the id of the
/// stream the store holds for it afterwards.
fn start_ingest(ivs: &Ivs, channel_arn: &str) -> String {
    ivs.with_store_mut(|store| {
        state::start_stream(store, channel_arn)
            .stream_id
            .expect("a minted stream carries its id")
    })
}

/// End a broadcast the way a stopped ingest does: the entry stays in the store and
/// leaves the `LIVE` state (`OFFLINE` is the other value the modelled `StreamState`
/// enum carries).
fn end_ingest(ivs: &Ivs, channel_arn: &str) {
    ivs.with_store_mut(|store| {
        let stream = store
            .streams
            .get_mut(channel_arn)
            .expect("the channel is broadcasting");
        stream.state = Some("OFFLINE".to_string());
    });
}

/// Whether `value` satisfies the MODELLED `StreamId` — exactly 26 characters matching
/// `^st-[a-zA-Z0-9]+$` (`smithy_query(query="shape",shape="StreamId")`) — which is what
/// decides whether the framework's constraint walk refuses a value before a handler sees it.
///
/// The one reading of that shape in this suite: both the minted ids the reads report and the
/// values the stream-id rows send are judged by it.
fn stream_id_is_well_formed(id: &str) -> bool {
    id.len() == 26
        && id
            .strip_prefix("st-")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Whether any string the error body carries names `arn`.
///
/// Which wire key the modelled `exceptionMessage` member renders under is a
/// separately-filed defect of this service's error serializer, so this looks at the
/// whole error object rather than pinning one key.
fn error_mentions(body: &Value, arn: &str) -> bool {
    body.as_object().is_some_and(|members| {
        members
            .values()
            .any(|value| value.as_str().is_some_and(|text| text.contains(arn)))
    })
}

/// `startTime` as seconds since the epoch.
///
/// The modelled `StreamStartTime` carries `@timestampFormat("date-time")`, which a
/// client reads as an RFC3339 string; this emulator's restJson1 serializer renders
/// the protocol's default epoch-seconds number for it. Both are the same instant, and
/// which one is on the wire is not what this suite is about, so either is accepted —
/// what is asserted is that the value IS the instant the ingest started.
fn start_time_secs(case: &str, stream: &Value) -> f64 {
    match stream.get("startTime") {
        Some(Value::String(rendered)) => DateTime::from_str(rendered, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: startTime {rendered:?} must be an RFC3339 instant, got {error}"
                )
            })
            .as_secs_f64(),
        Some(Value::Number(epoch)) => epoch.as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: startTime {epoch} must be a finite instant")
        }),
        other => panic!("case {case:?}: a live stream must carry startTime, got {other:?}"),
    }
}

/// Now, in the same units as [`start_time_secs`].
fn now_secs() -> f64 {
    DateTime::from(std::time::SystemTime::now()).as_secs_f64()
}

/// What the store holds for the channel a case reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Broadcast {
    /// The channel exists and nothing has ever ingested on it.
    Never,
    /// An ingest session is live on the channel.
    Live,
    /// An ingest session ran and ended, so the entry has left `LIVE`.
    Ended,
}

/// Assert the whole live-stream representation `GetStream` answered for a channel.
fn assert_live_stream(case: &str, reply: &Reply, channel: &Value, expected_id: &str) {
    let arn = text(channel, "arn");
    assert_eq!(
        reply.status, 200,
        "case {case:?}: a broadcasting channel must answer HTTP 200, got {} {}",
        reply.status, reply.body
    );
    let stream = reply
        .body
        .get("stream")
        .unwrap_or_else(|| panic!("case {case:?}: expected a stream member, got {}", reply.body));
    assert_eq!(
        text(stream, "channelArn"),
        arn,
        "case {case:?}: the stream must name the channel it is ingested on, got {stream}"
    );
    assert_eq!(
        text(stream, "streamId"),
        expected_id,
        "case {case:?}: the stream the store holds must be the one reported, got {stream}"
    );
    assert!(
        stream_id_is_well_formed(expected_id),
        "case {case:?}: streamId must be 26 characters matching ^st-[a-zA-Z0-9]+$, got {expected_id:?}"
    );
    assert_eq!(
        text(stream, "playbackUrl"),
        text(channel, "playbackUrl"),
        "case {case:?}: the stream is played back from its channel's own URL, got {stream}"
    );
    assert_eq!(
        text(stream, "state"),
        "LIVE",
        "case {case:?}: a reported stream is in the LIVE state, got {stream}"
    );
    assert_eq!(
        text(stream, "health"),
        "HEALTHY",
        "case {case:?}: a connected ingest reports HEALTHY, got {stream}"
    );
    assert_eq!(
        stream.get("viewerCount").and_then(Value::as_i64),
        Some(0),
        "case {case:?}: no playback session is attached, so viewerCount is 0, got {stream}"
    );
    let started = start_time_secs(case, stream);
    let elapsed = now_secs() - started;
    assert!(
        (-5.0..300.0).contains(&elapsed),
        "case {case:?}: startTime must be the instant the ingest connected, {elapsed} seconds off, got {stream}"
    );
}

/// Assert `GetStream` refused a channel that exists but is not broadcasting.
fn assert_not_broadcasting(case: &str, reply: &Reply, arn: &str) {
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (404, "ChannelNotBroadcasting"),
        "case {case:?}: expected 404 ChannelNotBroadcasting, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert!(
        error_mentions(&reply.body, arn),
        "case {case:?}: the refusal must name the channel asked about, got {}",
        reply.body
    );
    assert!(
        reply.body.get("stream").is_none(),
        "case {case:?}: a refusal carries no stream, got {}",
        reply.body
    );
}

/// The rule over a table spanning the channel surface and all three things the
/// store can hold for a channel: a live stream is reported member for member, and a
/// channel with no live stream is `ChannelNotBroadcasting` whether it never
/// broadcast or has stopped.
#[tokio::test]
async fn a_channel_answers_only_the_live_stream_the_store_holds_for_it() {
    // Rows differ along what the rule generalises over: the shape of the channel
    // (which members the create sent, its type, its latency mode, its tag count)
    // and which of the three broadcast conditions the store is in for it.
    let cases: Vec<(&str, Value, Broadcast)> = vec![
        ("every member defaulted, never ingested", json!({}), Broadcast::Never),
        (
            "named standard channel, live",
            json!({ "name": "live-standard" }),
            Broadcast::Live,
        ),
        (
            "basic channel, normal latency, live",
            json!({ "name": "live_basic", "type": "BASIC", "latencyMode": "NORMAL" }),
            Broadcast::Live,
        ),
        (
            "advanced-hd channel with several tags, live",
            json!({ "name": "live-advanced-hd", "type": "ADVANCED_HD",
                    "tags": { "env": "test", "team": "video" } }),
            Broadcast::Live,
        ),
        (
            "advanced-sd fragmented-mp4 channel whose ingest ended",
            json!({ "name": "ended-advanced-sd", "type": "ADVANCED_SD",
                    "containerFormat": "FRAGMENTED_MP4" }),
            Broadcast::Ended,
        ),
        (
            "authorized channel, never ingested",
            json!({ "name": "private", "authorized": true }),
            Broadcast::Never,
        ),
    ];

    let ivs = Ivs::new();
    // Every stream the store mints is a distinct resource, so no two channels may
    // ever be reported under one stream id.
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, request, broadcast) in cases {
        let created = ivs.create(request).await;
        let channel = created
            .get("channel")
            .unwrap_or_else(|| panic!("case {case:?}: CreateChannel returns a channel"))
            .clone();
        let arn = text(&channel, "arn");
        assert!(
            arn.starts_with(CHANNEL_ARN_PREFIX),
            "case {case:?}: a minted channel ARN carries the modelled prefix, got {arn:?}"
        );

        let started = match broadcast {
            Broadcast::Never => None,
            Broadcast::Live => Some(start_ingest(&ivs, &arn)),
            Broadcast::Ended => {
                let id = start_ingest(&ivs, &arn);
                end_ingest(&ivs, &arn);
                Some(id)
            }
        };
        if let Some(id) = &started {
            assert!(
                minted.insert(id.clone()),
                "case {case:?}: stream id {id:?} was already minted for another channel"
            );
        }

        let reply = get_stream(&ivs, &arn).await;
        match broadcast {
            Broadcast::Live => assert_live_stream(
                case,
                &reply,
                &channel,
                started.as_deref().expect("a live case started an ingest"),
            ),
            Broadcast::Never | Broadcast::Ended => assert_not_broadcasting(case, &reply, &arn),
        }
    }
}

/// A channel ARN the store does not hold is `ResourceNotFoundException`, not
/// `ChannelNotBroadcasting`: the channel is resolved before its stream is, so a
/// well-formed ARN that was never minted, and one whose channel was deleted, answer
/// the same way.
#[tokio::test]
async fn a_channel_arn_the_store_does_not_hold_is_not_found() {
    let ivs = Ivs::new();
    // A channel that IS broadcasting, so no case below can be answered out of some
    // other channel's stream by accident.
    let bystander = text(
        ivs.create(json!({ "name": "bystander" }))
            .await
            .get("channel")
            .expect("CreateChannel returns a channel"),
        "arn",
    );
    start_ingest(&ivs, &bystander);

    // A deleted channel, and a deleted channel that was broadcasting when it went:
    // both are gone, stream and all.
    let deleted = text(
        ivs.create(json!({ "name": "deleted" }))
            .await
            .get("channel")
            .expect("CreateChannel returns a channel"),
        "arn",
    );
    delete_channel(&ivs, &deleted).await;
    let deleted_live = text(
        ivs.create(json!({ "name": "deleted-live" }))
            .await
            .get("channel")
            .expect("CreateChannel returns a channel"),
        "arn",
    );
    start_ingest(&ivs, &deleted_live);
    delete_channel(&ivs, &deleted_live).await;

    // Rows differ along what the rule generalises over: how the ARN came to name no
    // channel (never minted, deleted, deleted while live) and its form within the
    // modelled pattern — region, account, id length and id alphabet, including an
    // ARN exactly at the modelled 128-character bound.
    let at_the_bound = format!(
        "{CHANNEL_ARN_PREFIX}{}",
        "a".repeat(CHANNEL_ARN_MAX - CHANNEL_ARN_PREFIX.len())
    );
    let cases: Vec<(&str, String)> = vec![
        ("never minted, all-zero id", format!("{CHANNEL_ARN_PREFIX}000000000000")),
        (
            "never minted, another region",
            "arn:aws:ivs:eu-west-1:000000000000:channel/abcdef123456".to_string(),
        ),
        (
            "never minted, another account",
            "arn:aws:ivs:us-east-1:123456789012:channel/abcdef123456".to_string(),
        ),
        (
            "never minted, single-character id",
            format!("{CHANNEL_ARN_PREFIX}a"),
        ),
        ("never minted, at the 128-character bound", at_the_bound),
        (
            "never minted, dashed mixed-case id",
            format!("{CHANNEL_ARN_PREFIX}Live-Channel-01"),
        ),
        ("deleted after creation", deleted),
        ("deleted while broadcasting", deleted_live),
    ];

    for (case, arn) in cases {
        let reply = get_stream(&ivs, &arn).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException for {arn}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert!(
            error_mentions(&reply.body, &arn),
            "case {case:?}: the refusal must name the ARN asked for, got {}",
            reply.body
        );
    }

    // And the channel that is live is untouched by every refusal above.
    let reply = get_stream(&ivs, &bystander).await;
    assert_eq!(
        reply.status, 200,
        "case bystander: the broadcasting channel still reports its stream, got {} {}",
        reply.status, reply.body
    );
}

/// An ARN outside the modelled `ChannelArn` constraint (`@length 1..128`, pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:channel/[a-zA-Z0-9-]+$`) is a bad REQUEST, so it
/// is refused with 400 `ValidationException` rather than reported as a missing
/// resource.
#[tokio::test]
async fn a_channel_arn_outside_the_modelled_constraint_is_refused() {
    let ivs = Ivs::new();
    // Rows: each bound of the modelled length one step past it, and each part of the
    // pattern violated in turn — resource type, alphabet, and the ARN form itself.
    let past_the_bound = format!(
        "{CHANNEL_ARN_PREFIX}{}",
        "a".repeat(CHANNEL_ARN_MAX + 1 - CHANNEL_ARN_PREFIX.len())
    );
    let cases: Vec<(&str, String)> = vec![
        ("empty, below the 1-character minimum", String::new()),
        ("one past the 128-character maximum", past_the_bound),
        (
            "a stream-key ARN, not a channel ARN",
            "arn:aws:ivs:us-east-1:000000000000:stream-key/abcdef123456".to_string(),
        ),
        (
            "an id carrying a character outside [a-zA-Z0-9-]",
            format!("{CHANNEL_ARN_PREFIX}chan_01"),
        ),
        ("not an ARN at all", "live-standard".to_string()),
        (
            "an account segment that is not digits",
            "arn:aws:ivs:us-east-1:not-an-account:channel/abcdef123456".to_string(),
        ),
    ];

    for (case, arn) in cases {
        let reply = get_stream(&ivs, &arn).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException for {arn:?}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// `GetStream` is a REPORT: polling it answers the same stream every time and moves
/// nothing — not the stream id, not the start time, not the viewer count — and a
/// channel that is not broadcasting never becomes live by being read. Each channel
/// reports only its own stream.
#[tokio::test]
async fn repeated_reads_report_the_same_stream_and_change_nothing() {
    let ivs = Ivs::new();
    // Rows differ in whether the channel is broadcasting and in how many channels
    // sit beside it, which is what the "only its own stream" half of the rule
    // generalises over.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("first live channel", json!({ "name": "live-a" }), true),
        ("quiet channel between two live ones", json!({ "name": "quiet-b" }), false),
        (
            "second live channel",
            json!({ "name": "live-c", "type": "BASIC" }),
            true,
        ),
        ("quiet advanced channel", json!({ "name": "quiet-d", "type": "ADVANCED_SD" }), false),
    ];

    let mut seeded: Vec<(&str, Value, Option<String>)> = Vec::new();
    for (case, request, live) in cases {
        let created = ivs.create(request).await;
        let channel = created
            .get("channel")
            .unwrap_or_else(|| panic!("case {case:?}: CreateChannel returns a channel"))
            .clone();
        let arn = text(&channel, "arn");
        let id = live.then(|| start_ingest(&ivs, &arn));
        seeded.push((case, channel, id));
    }

    // Four rounds of polling, the way a client waits for a channel to go live.
    let mut first: Vec<Option<Value>> = Vec::new();
    for round in 0..4 {
        for (index, (case, channel, id)) in seeded.iter().enumerate() {
            let arn = text(channel, "arn");
            let reply = get_stream(&ivs, &arn).await;
            let label = format!("{case} round {round}");
            match id {
                Some(id) => {
                    assert_live_stream(&label, &reply, channel, id);
                    let stream = reply.body.get("stream").cloned();
                    if round == 0 {
                        first.push(stream);
                    } else {
                        assert_eq!(
                            first.get(index).cloned().flatten(),
                            stream,
                            "case {label:?}: a read must report the same stream it reported before, got {}",
                            reply.body
                        );
                    }
                }
                None => {
                    assert_not_broadcasting(&label, &reply, &arn);
                    if round == 0 {
                        first.push(None);
                    }
                }
            }
        }
    }
}

/// A channel holds at most one stream: a second ingest on a channel that is already
/// live keeps the stream it has rather than minting a second id, so what `GetStream`
/// reports does not change under it.
#[tokio::test]
async fn a_second_ingest_on_a_live_channel_keeps_the_stream_it_has() {
    let ivs = Ivs::new();
    let created = ivs
        .create(json!({ "name": "re-ingested", "type": "STANDARD" }))
        .await;
    let channel = created
        .get("channel")
        .expect("CreateChannel returns a channel")
        .clone();
    let arn = text(&channel, "arn");

    let first = start_ingest(&ivs, &arn);
    let before = get_stream(&ivs, &arn).await;
    assert_live_stream("first ingest", &before, &channel, &first);

    // Rows: two further ingest attempts on the same live channel.
    for attempt in 1..=2 {
        let again = start_ingest(&ivs, &arn);
        assert_eq!(
            again, first,
            "case ingest attempt {attempt}: a live channel keeps its stream id, got {again:?}"
        );
        let after = get_stream(&ivs, &arn).await;
        assert_live_stream(&format!("ingest attempt {attempt}"), &after, &channel, &first);
        assert_eq!(
            after.body.get("stream"),
            before.body.get("stream"),
            "case ingest attempt {attempt}: the reported stream must be unchanged, got {}",
            after.body
        );
    }
}

// =============================================================================
// The CONTRIBUTE plane: a real publish on the address a channel announces
// =============================================================================
//
// The rule these cases assert, stated once:
//
// The address a channel announces as its ingest endpoint is an address THIS emulator
// answers RTMP on, and a publish authenticated by a stream key this deployment minted
// brings that key's own channel live — which is what produces every fact a live IVS
// broadcast is read by: `GetStream` reporting `LIVE` for that channel, a session carrying
// the instant the channel went live, an HLS multivariant playlist at the announced
// `playbackUrl` once the first segments are packaged, and the unwind of all three when the
// broadcast ends (`StopStream` disconnecting the publisher, the session gaining the instant
// the channel went offline). A publish presenting a stream key no channel minted is refused
// on the wire and leaves every channel offline.
//
// Each case drives a REAL publisher over a REAL socket, dialing exactly the value the
// create announced, with the same handshake, commands and media messages an encoder sends.

/// How long a wait on a CONDITION may take before the case gives up.
///
/// Generous on purpose: it bounds the emulator having served a step, not the time any step
/// is expected to take, so a loaded host cannot fail a case that a fast one passes.
const CONDITION_DEADLINE: Duration = Duration::from_secs(30);

/// How often a waiting case looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How long one read of the publisher's socket may block before the case gives up on it,
/// for the same reason.
const READ_DEADLINE: Duration = Duration::from_secs(30);

/// The application an encoder dials under the ingest host, which is the path AWS documents
/// for an IVS contribution (`rtmps://<ingestEndpoint>:443/app/<streamKey>`).
const CONTRIBUTE_APPLICATION: &str = "app";

/// The media an encoder sends, as the FLV packaging an RTMP publisher carries it in: the
/// frame type and codec in the first byte, the packet type in the second. The first message
/// of each wire is the decoder configuration, and the frames follow.
const VIDEO_CONFIGURATION: &[u8] = &[0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x00, 0x2a];
/// A video keyframe, which is where a segment may begin.
const VIDEO_KEYFRAME: &[u8] = &[0x17, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x65];
/// A video frame that is not a keyframe.
const VIDEO_FRAME: &[u8] = &[0x27, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x41];
/// The audio decoder configuration an AAC publisher sends.
const AUDIO_CONFIGURATION: &[u8] = &[0xaf, 0x00, 0x12, 0x10];
/// One audio frame.
const AUDIO_FRAME: &[u8] = &[0xaf, 0x01, 0x21, 0x10, 0x04, 0x60];

/// What a dialled contribute connection is: a plaintext socket, or the TLS stream over one
/// — the two wires this plane serves on the one address it announces.
trait Dialled: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Dialled for T {}

/// The client side of the TLS an `rtmps://` publisher opens: the certificate is the one the
/// core generated for its own listeners — self-signed, and this client is the emulator's own
/// test dialling a port this very process opened — so there is no authority to check it
/// against. An encoder dialling `rtmps://` out of this process is in the same position.
#[derive(Debug)]
struct AcceptTheEmulatorsCertificate(std::sync::Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptTheEmulatorsCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Open a connection to the announced contribute `authority`, encrypted when the publisher
/// dialled `rtmps://` and in the clear when it dialled `rtmp://`.
///
/// The port is the one the announced value itself carries — all a publisher outside this
/// process holds — and the host it names is this machine's own loopback interface, which is
/// where the core binds the plane's socket and what `assert_dialable_authority` has already
/// checked the announced host to be.
async fn dial_contribute(authority: &str, encrypted: bool) -> Result<Box<dyn Dialled>, String> {
    let port = authority_port(authority);
    let socket = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|error| {
            format!("the contribute socket {authority} must accept a connection: {error}")
        })?;
    if !encrypted {
        return Ok(Box::new(socket));
    }
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("the provider supports the default protocol versions: {error}"))?
        .dangerous()
        .with_custom_certificate_verifier(std::sync::Arc::new(AcceptTheEmulatorsCertificate(
            provider,
        )))
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
    let server_name =
        rustls::pki_types::ServerName::try_from("localhost").expect("localhost is a server name");
    let encrypted = connector
        .connect(server_name, socket)
        .await
        .map_err(|error| {
            format!("an rtmps publisher's TLS handshake to {authority} must complete: {error}")
        })?;
    Ok(Box::new(encrypted))
}

/// One encoder publishing to the address a channel announced, over a real socket.
struct Publisher {
    /// The connection to the announced contribute address, encrypted or in the clear.
    conn: Box<dyn Dialled>,
    /// The client half of the RTMP session, which is what writes the publisher's bytes.
    session: rml_rtmp::sessions::ClientSession,
    /// Every byte the ingest server has sent on this connection. A refused publish is
    /// answered with an RTMP status the client session carries no event for, so the status
    /// is read off the wire itself ([`stated_refusal`]).
    seen: Vec<u8>,
}

impl Publisher {
    /// Dial the announced `authority` IN THE CLEAR and publish `stream_key`, the way an
    /// encoder configured with a channel's ingest endpoint and stream key does over
    /// `rtmp://`, or `Err` carrying what the ingest server said when it refused the publish
    /// (or when the connection ended without one).
    async fn publishing(authority: &str, stream_key: &str) -> Result<Self, String> {
        Self::publishing_over(authority, stream_key, false).await
    }

    /// The same publish over TLS, which is what an encoder pointed at
    /// `rtmps://<ingestEndpoint>/app/<streamKey>` opens.
    async fn publishing_over_tls(authority: &str, stream_key: &str) -> Result<Self, String> {
        Self::publishing_over(authority, stream_key, true).await
    }

    /// Dial the announced `authority` and publish `stream_key`, encrypted or in the clear.
    async fn publishing_over(
        authority: &str,
        stream_key: &str,
        encrypted: bool,
    ) -> Result<Self, String> {
        use rml_rtmp::sessions::{ClientSession, ClientSessionConfig, PublishRequestType};

        // The contribute socket is bound on one of this machine's own interfaces, so the
        // encoder's connection is this emulator's own loopback port.
        let mut conn = dial_contribute(authority, encrypted).await?;
        // The one handshake of this crate's suites, shared with the suite that opens an
        // in-process connection to an address composed from the announced host.
        let leftover = super::channel_lifecycle::rtmp_handshake(&mut conn).await?;
        let (session, initial) = ClientSession::new(ClientSessionConfig::new())
            .map_err(|error| format!("the client session must start: {error:?}"))?;
        let mut publisher = Self {
            conn,
            session,
            seen: Vec::new(),
        };
        publisher.carry_out(initial).await?;
        if !leftover.is_empty() {
            let results = publisher
                .session
                .handle_input(&leftover)
                .map_err(|error| format!("the session must read what followed the handshake: {error:?}"))?;
            publisher.carry_out(results).await?;
        }
        let packet = publisher
            .session
            .request_connection(CONTRIBUTE_APPLICATION.to_string())
            .map_err(|error| format!("the connect command must be built: {error:?}"))?;
        publisher.send(packet).await?;
        publisher.wait_for_connection().await?;
        let packet = publisher
            .session
            .request_publishing(stream_key.to_string(), PublishRequestType::Live)
            .map_err(|error| format!("the publish command must be built: {error:?}"))?;
        publisher.send(packet).await?;
        publisher.wait_for_publish().await?;
        Ok(publisher)
    }

    /// Tell the ingest server what this encoder is sending, as an encoder's own metadata
    /// does.
    async fn send_metadata(&mut self, width: u32, height: u32) -> Result<(), String> {
        let metadata = rml_rtmp::sessions::StreamMetadata {
            video_width: Some(width),
            video_height: Some(height),
            video_codec_id: Some(7),
            video_frame_rate: Some(30.0),
            video_bitrate_kbps: Some(2_500),
            audio_codec_id: Some(10),
            audio_bitrate_kbps: Some(128),
            audio_sample_rate: Some(44_100),
            audio_channels: Some(2),
            audio_is_stereo: Some(true),
            encoder: Some("sim-encoder".to_string()),
        };
        let packet = self
            .session
            .publish_metadata(&metadata)
            .map_err(|error| format!("the metadata must be built: {error:?}"))?;
        self.send(packet).await
    }

    /// Send the decoder configuration of each wire, which is what an encoder sends before
    /// its first frame.
    async fn send_configuration(&mut self) -> Result<(), String> {
        self.send_video(VIDEO_CONFIGURATION, 0).await?;
        self.send_audio(AUDIO_CONFIGURATION, 0).await
    }

    /// Send `seconds` seconds of media, a keyframe every second, which is what gives the
    /// playback plane whole segments to package.
    async fn send_media(&mut self, from: u32, seconds: u32) -> Result<(), String> {
        for second in 0..seconds {
            let at = (from + second) * 1_000;
            self.send_video(VIDEO_KEYFRAME, at).await?;
            self.send_audio(AUDIO_FRAME, at + 250).await?;
            self.send_video(VIDEO_FRAME, at + 500).await?;
            self.send_audio(AUDIO_FRAME, at + 750).await?;
        }
        Ok(())
    }

    /// One video message at `timestamp` milliseconds.
    async fn send_video(&mut self, payload: &[u8], timestamp: u32) -> Result<(), String> {
        let packet = self
            .session
            .publish_video_data(
                bytes::Bytes::from(payload.to_vec()),
                rml_rtmp::time::RtmpTimestamp::new(timestamp),
                false,
            )
            .map_err(|error| format!("the video message must be built: {error:?}"))?;
        self.send(packet).await
    }

    /// One audio message at `timestamp` milliseconds.
    async fn send_audio(&mut self, payload: &[u8], timestamp: u32) -> Result<(), String> {
        let packet = self
            .session
            .publish_audio_data(
                bytes::Bytes::from(payload.to_vec()),
                rml_rtmp::time::RtmpTimestamp::new(timestamp),
                false,
            )
            .map_err(|error| format!("the audio message must be built: {error:?}"))?;
        self.send(packet).await
    }

    /// Write one packet the session produced.
    async fn send(&mut self, packet: rml_rtmp::sessions::ClientSessionResult) -> Result<(), String> {
        self.carry_out(vec![packet]).await
    }

    /// Write every response the session produced, and read past the events it raised.
    async fn carry_out(
        &mut self,
        results: Vec<rml_rtmp::sessions::ClientSessionResult>,
    ) -> Result<(), String> {
        for result in results {
            if let rml_rtmp::sessions::ClientSessionResult::OutboundResponse(packet) = result {
                let bytes = packet.bytes.clone();
                write_all(&mut self.conn, &bytes).await?;
            }
        }
        Ok(())
    }

    /// Read until the ingest server has accepted the connection, or carried back its own
    /// refusal of it.
    async fn wait_for_connection(&mut self) -> Result<(), String> {
        self.wait_for(|event| match event {
            rml_rtmp::sessions::ClientSessionEvent::ConnectionRequestAccepted => Some(Ok(())),
            rml_rtmp::sessions::ClientSessionEvent::ConnectionRequestRejected { description } => {
                Some(Err(format!(
                    "the ingest server refused the connection: {description}"
                )))
            }
            _ => None,
        })
        .await
    }

    /// Read until the ingest server has accepted the publish, or TOLD this publisher why it
    /// will not take it — which is what a refusal is on this wire, and so is what the error
    /// carries ([`stated_refusal`]).
    async fn wait_for_publish(&mut self) -> Result<(), String> {
        self.wait_for(|event| match event {
            rml_rtmp::sessions::ClientSessionEvent::PublishRequestAccepted => Some(Ok(())),
            _ => None,
        })
        .await
    }

    /// Read the connection until an event `read` answers for arrives, answering every
    /// response the session produces on the way; `Err` when that event is a refusal, when
    /// the server stated a refusal and closed, or when nothing arrived within the deadline.
    async fn wait_for(
        &mut self,
        read: impl Fn(&rml_rtmp::sessions::ClientSessionEvent) -> Option<Result<(), String>>,
    ) -> Result<(), String> {
        let deadline = std::time::Instant::now() + CONDITION_DEADLINE;
        while std::time::Instant::now() < deadline {
            let arriving = match read_some(&mut self.conn).await {
                Ok(arriving) => arriving,
                // The connection ended. When the server STATED a refusal before closing,
                // that status is the answer this publisher got.
                Err(ended) => {
                    return Err(match stated_refusal(&self.seen) {
                        Some(stated) => stated,
                        None => ended,
                    })
                }
            };
            self.seen.extend_from_slice(&arriving);
            let results = self
                .session
                .handle_input(&arriving)
                .map_err(|error| format!("the ingest server's answer must be read: {error:?}"))?;
            let mut answer = None;
            for result in results {
                match result {
                    rml_rtmp::sessions::ClientSessionResult::OutboundResponse(packet) => {
                        let bytes = packet.bytes.clone();
                        write_all(&mut self.conn, &bytes).await?;
                    }
                    rml_rtmp::sessions::ClientSessionResult::RaisedEvent(event) => {
                        if let Some(read) = read(&event) {
                            answer = Some(read);
                        }
                    }
                    rml_rtmp::sessions::ClientSessionResult::UnhandleableMessageReceived(_) => {}
                }
            }
            if let Some(answer) = answer {
                return answer;
            }
            // A refusal the session raises no event for, stated while the connection is
            // still open.
            if let Some(stated) = stated_refusal(&self.seen) {
                return Err(stated);
            }
        }
        Err("the ingest server did not answer within the deadline".to_string())
    }
}

/// How a refusal the ingest server STATED is reported back by the publisher above, ahead of
/// the status that server sent: the status is the refusal, so a case can tell it apart from
/// a connection that merely ended.
const REFUSED_BY_THE_SERVER: &str = "the ingest server refused the publish: ";

/// The RTMP status an ingest server refuses a publish with when the stream name presented
/// is not one it may publish, and the text beside it — the two AMF0 strings the status
/// message carries, which is what an encoder reports to its operator.
const PUBLISH_REFUSED_STATUS: [&str; 2] = [
    "NetStream.Publish.BadName",
    "The stream key presented is not a stream key of any channel",
];

/// The refusal the ingest server stated in `seen`, when it stated one: the status strings
/// it sent, read out of the bytes themselves because the client session raises no event for
/// a rejected publish. `None` when no refusal was sent.
fn stated_refusal(seen: &[u8]) -> Option<String> {
    let stated: Vec<&str> = PUBLISH_REFUSED_STATUS
        .into_iter()
        .filter(|needle| carries(seen, needle.as_bytes()))
        .collect();
    (!stated.is_empty()).then(|| format!("{REFUSED_BY_THE_SERVER}{}", stated.join(" / ")))
}

/// Whether `haystack` carries `needle`.
fn carries(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Read some bytes from `conn`, bounded by a generous deadline, or `Err` when the server
/// closed the connection — which is how a refused publish ends.
async fn read_some(conn: &mut Box<dyn Dialled>) -> Result<Vec<u8>, String> {
    let mut buffer = [0_u8; 4096];
    let read = tokio::time::timeout(READ_DEADLINE, conn.read(&mut buffer))
        .await
        .map_err(|_| "the ingest server did not answer within the deadline".to_string())?
        .map_err(|error| format!("the ingest connection must be readable: {error}"))?;
    match read {
        0 => Err("the ingest server closed the connection".to_string()),
        read => Ok(buffer[..read].to_vec()),
    }
}

/// Write `bytes` to `conn`.
async fn write_all(conn: &mut Box<dyn Dialled>, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Ok(());
    }
    conn.write_all(bytes)
        .await
        .map_err(|error| format!("the ingest connection must be writable: {error}"))
}

/// Whether the ingest server has CLOSED this publisher's connection — a read that answers
/// end of file, which is what a disconnected ingest session is on the wire.
///
/// Bytes the server writes before closing (the session's own bookkeeping) are read past, and
/// a connection that stays open until the deadline is not severed.
async fn severed(conn: &mut Box<dyn Dialled>) -> bool {
    for _read in 0..8 {
        let mut buffer = [0_u8; 4096];
        match tokio::time::timeout(READ_DEADLINE, conn.read(&mut buffer)).await {
            Ok(Ok(0)) | Ok(Err(_)) => return true,
            Ok(Ok(_)) => continue,
            Err(_deadline) => return false,
        }
    }
    false
}

/// Look again until `condition` holds, or fail the case once a generous deadline has
/// passed: what a caller of a live broadcast waits on is the CONDITION, never a duration.
async fn poll_until<C, F>(case: &str, waiting_for: &str, mut condition: C)
where
    C: FnMut() -> F,
    F: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + CONDITION_DEADLINE;
    loop {
        if condition().await {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: expected {waiting_for} before the deadline"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The ADDRESS a publisher dials this deployment's contribute plane at, and the stream key
/// the create minted for the channel it publishes to — both read off the create's own
/// response, which is everything a broadcaster holds.
///
/// The channel's announced `ingestEndpoint` IS that address: the `host:port` the core bound
/// the plane's socket at, which an encoder dials as it stands
/// (`rtmps://<ingestEndpoint>/app/<streamKey>`). The plane's own record of where it listens
/// is asserted to be that same value, so these cases drive what a client is told rather than
/// a deployment detail they looked up.
fn contribute_target(ivs: &Ivs, created: &Value) -> (String, String) {
    let announced = text(&created["channel"], "ingestEndpoint");
    assert_eq!(
        announced,
        ivs.contribute_socket(),
        "the announced ingestEndpoint must be the address this deployment's contribute plane \
         answers on, got {announced:?}"
    );
    (announced, text(&created["streamKey"], "value"))
}

/// The port one `host:port` authority names, read through the workspace's one reading of an
/// authority.
///
/// Shared with `announced_channel_endpoints.rs`, which asserts the same announced value's
/// form, rather than kept twice in one test binary.
pub(crate) fn authority_port(authority: &str) -> u16 {
    ::service_common::data_plane_addressing::addressed_port(Some(authority))
        .and_then(|port| u16::try_from(port).ok())
        .unwrap_or_else(|| panic!("a dialable authority carries a port, got {authority:?}"))
}

/// RULE, first leg: a publish on the announced contribute address, authenticated by a
/// channel's own stream key, brings THAT channel live — and only that channel.
///
/// Rows differ along what the rule generalises over: the channel's type and latency mode,
/// whether it is tagged, which of the account's channels the key belongs to, and a key
/// minted by `CreateStreamKey` after the channel was created rather than by the create
/// itself.
#[tokio::test]
async fn a_publish_on_the_announced_address_brings_its_own_channel_live() {
    let ivs = Ivs::new();
    let creates: Vec<(&str, Value)> = vec![
        ("the issue's own channel", json!({ "name": "live-1", "latencyMode": "NORMAL", "type": "STANDARD" })),
        ("a basic channel with insecure ingest", json!({ "name": "live-2", "type": "BASIC", "insecureIngest": true })),
        ("an advanced channel, tagged", json!({ "name": "live-3", "type": "ADVANCED_HD", "tags": { "env": "test" } })),
        ("every member defaulted", json!({})),
    ];
    let mut channels: Vec<(String, String, String)> = Vec::new();
    // The publishers are kept alive for the whole case: a channel is live exactly while
    // something is publishing to it, so dropping one here would end its broadcast.
    let mut publishers: Vec<Publisher> = Vec::new();
    for (case, request) in creates {
        let created = ivs.create(request).await;
        let (ingest, key) = contribute_target(&ivs, &created);
        channels.push((case.to_string(), text(&created["channel"], "arn"), key.clone()));

        // Nothing is publishing yet, so nothing is live.
        let offline = get_stream(&ivs, &channels.last().expect("just pushed").1).await;
        assert_eq!(
            offline.status, 404,
            "case {case:?}: a channel nothing publishes to must not be broadcasting, got {} {}",
            offline.status, offline.body
        );

        let mut publisher = Publisher::publishing(&ingest, &key)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: {error}"));
        publisher
            .send_metadata(1_280, 720)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: {error}"));

        let arn = channels.last().expect("just pushed").1.clone();
        poll_until(case, "GetStream to report the channel LIVE", || {
            let ivs = &ivs;
            let arn = arn.clone();
            async move {
                let reply = get_stream(ivs, &arn).await;
                reply.status == 200
                    && text(&reply.body["stream"], "state") == "LIVE"
                    && text(&reply.body["stream"], "channelArn") == arn
            }
        })
        .await;

        // The stream the publish brought up is the channel's own, with the modelled id and
        // the instant it started.
        let live = get_stream(&ivs, &arn).await;
        let stream = &live.body["stream"];
        let stream_id = text(stream, "streamId");
        assert!(
            stream_id_is_well_formed(&stream_id),
            "case {case:?}: the publish must mint a modelled StreamId, got {stream_id:?}"
        );
        assert!(
            stream.get("startTime").is_some(),
            "case {case:?}: a live stream carries the instant it started, got {stream}"
        );

        // And the session that broadcast is leaving behind is readable while it runs: it
        // reports when the channel went live and, being live, no end. The encoder's own
        // metadata is what it reports as the ingest configuration, so nothing about the
        // stream is invented — and that is a CONDITION the publisher's own metadata
        // message produces, so the case waits for it rather than for a duration.
        poll_until(case, "the session to report the publisher's own metadata", || {
            let ivs = &ivs;
            let arn = arn.clone();
            async move {
                let reply = ivs
                    .call("GetStreamSession", &json!({ "channelArn": arn }))
                    .await;
                reply.body["streamSession"]["ingestConfiguration"]["video"]["videoWidth"]
                    .as_i64()
                    == Some(1_280)
            }
        })
        .await;
        let session = ivs
            .call("GetStreamSession", &json!({ "channelArn": arn }))
            .await;
        assert_eq!(
            session.status, 200,
            "case {case:?}: GetStreamSession must report the running broadcast, got {} {}",
            session.status, session.body
        );
        let reported = &session.body["streamSession"];
        assert_eq!(
            text(reported, "streamId"),
            stream_id,
            "case {case:?}: the session must be the running broadcast's, got {reported}"
        );
        assert!(
            reported.get("startTime").is_some() && reported.get("endTime").is_none(),
            "case {case:?}: a live session reports its start and no end, got {reported}"
        );
        assert_eq!(
            reported["ingestConfiguration"]["audio"]["sampleRate"]
                .as_i64()
                .unwrap_or_default(),
            44_100,
            "case {case:?}: the session reports the publisher's own audio rate, got {reported}"
        );
        publishers.push(publisher);
    }

    // Only the published channels are live, each with its own stream: a publish on one
    // channel's key never brings another's live.
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for (case, arn, _key) in &channels {
        let reply = get_stream(&ivs, arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: every published channel stays live, got {} {}",
            reply.status, reply.body
        );
        ids.insert(text(&reply.body["stream"], "streamId"));
    }
    assert_eq!(
        ids.len(),
        channels.len(),
        "each channel's broadcast is its own stream, got {ids:?}"
    );
    drop(publishers);
}

/// RULE, the refusal leg: a publish presenting a stream key no channel of this store minted
/// is refused on the wire and leaves every channel offline.
///
/// Rows differ along what the rule generalises over: a key no channel ever had, a key of
/// the right shape that was deleted, the empty key, and the channel's own key with an
/// encoder's query appended — which IS the channel's key and so is admitted.
#[tokio::test]
async fn a_publish_presenting_a_key_no_channel_minted_is_refused() {
    let ivs = Ivs::new();
    let created = ivs.create(json!({ "name": "guarded" })).await;
    let (ingest, key) = contribute_target(&ivs, &created);
    let arn = text(&created["channel"], "arn");
    let deleted = ivs.create(json!({ "name": "deleted-key" })).await;
    let deleted_key = text(&deleted["streamKey"], "value");
    let deleted_key_arn = text(&deleted["streamKey"], "arn");
    let dropped = ivs
        .call("DeleteStreamKey", &json!({ "arn": deleted_key_arn }))
        .await;
    assert_eq!(
        dropped.status, 204,
        "DeleteStreamKey must answer the modelled 204, got {} {}",
        dropped.status, dropped.body
    );

    let rows: Vec<(&str, String, bool)> = vec![
        ("a key no channel ever had", "sk_us-east-1_notaknown_key".to_string(), false),
        ("a key that was deleted", deleted_key, false),
        ("no key at all", String::new(), false),
        ("the channel's own key with a query appended", format!("{key}?token=1"), true),
    ];
    for (case, presented, admitted) in rows {
        let publishing = Publisher::publishing(&ingest, &presented).await;
        assert_eq!(
            publishing.is_ok(),
            admitted,
            "case {case:?}: expected the publish to be {}, got {:?}",
            match admitted {
                true => "admitted",
                false => "refused",
            },
            publishing.as_ref().err()
        );
        // A refusal is a STATUS on the wire, not a bare close: the publisher is told the
        // code and the text for what it presented — no channel's stream key — which is
        // what an encoder reports to its operator.
        if let Err(refused) = publishing.as_ref() {
            for stated in PUBLISH_REFUSED_STATUS {
                assert!(
                    refused.contains(stated),
                    "case {case:?}: expected the ingest server to state {stated:?} when it \
                     refused the publish, got {refused:?}"
                );
            }
        }
        let reply = get_stream(&ivs, &arn).await;
        let live = reply.status == 200;
        assert_eq!(
            live, admitted,
            "case {case:?}: expected the channel to be {}, got {} {}",
            match admitted {
                true => "live",
                false => "offline",
            },
            reply.status,
            reply.body
        );
        if let Ok(publisher) = publishing {
            drop(publisher);
            // The channel goes offline again once its publisher is gone, which is what
            // makes the next row's assertion about an offline channel meaningful.
            poll_until(case, "the channel to go offline once its publisher is gone", || {
                let ivs = &ivs;
                let arn = arn.clone();
                async move { get_stream(ivs, &arn).await.status == 404 }
            })
            .await;
        }
    }
}

/// RULE, the playback leg: the `playbackUrl` a channel announces serves an HLS multivariant
/// playlist once the publisher's first segments are packaged, and serves nothing before
/// that or once the broadcast has ended.
#[tokio::test]
async fn the_announced_playback_url_serves_a_playlist_once_segments_are_packaged() {
    let ivs = Ivs::new();
    let created = ivs.create(json!({ "name": "played-back" })).await;
    let (ingest, key) = contribute_target(&ivs, &created);
    let arn = text(&created["channel"], "arn");
    let playback = text(&created["channel"], "playbackUrl");

    // Before anything publishes, the announced URL serves no playlist.
    let before = ivs.fetch(&playback).await;
    assert_eq!(
        before.status, 404,
        "a channel nothing publishes to serves no playlist, got {} {:?}",
        before.status, before.body
    );

    let mut publisher = Publisher::publishing(&ingest, &key)
        .await
        .unwrap_or_else(|error| panic!("the announced address must accept a publish: {error}"));
    publisher
        .send_metadata(640, 360)
        .await
        .expect("the metadata is sent");
    publisher
        .send_configuration()
        .await
        .expect("the decoder configuration is sent");
    publisher
        .send_media(0, 4)
        .await
        .expect("the media is sent");

    poll_until("a packaged broadcast", "the announced playbackUrl to serve a playlist", || {
        let ivs = &ivs;
        let playback = playback.clone();
        async move { ivs.fetch(&playback).await.status == 200 }
    })
    .await;

    let served = ivs.fetch(&playback).await;
    assert!(
        served.body.starts_with("#EXTM3U"),
        "the playback URL must serve an HLS playlist, got {:?}",
        served.body
    );
    assert_eq!(
        served.content_type, "application/vnd.apple.mpegurl",
        "an HLS playlist is served as one, got {:?}",
        served.content_type
    );
    assert!(
        served.body.contains("#EXT-X-STREAM-INF:"),
        "the announced URL must serve the MULTIVARIANT playlist, got {:?}",
        served.body
    );
    // The variant it names is fetchable at the same place, and so is every segment it
    // lists: a playlist that points at nothing is no playlist.
    let variant = served
        .body
        .lines()
        .find(|line| !line.starts_with('#') && !line.trim().is_empty())
        .unwrap_or_else(|| panic!("the multivariant playlist names a variant, got {:?}", served.body))
        .to_string();
    let media_url = sibling_url(&playback, &variant);
    let media = ivs.fetch(&media_url).await;
    assert_eq!(
        media.status, 200,
        "the variant the multivariant playlist names must be served, got {} {:?}",
        media.status, media.body
    );
    assert!(
        media.body.contains("#EXTINF:") && media.body.contains("#EXT-X-TARGETDURATION:"),
        "the media playlist lists the packaged segments, got {:?}",
        media.body
    );
    let segment = media
        .body
        .lines()
        .find(|line| !line.starts_with('#') && !line.trim().is_empty())
        .unwrap_or_else(|| panic!("the media playlist lists a segment, got {:?}", media.body))
        .to_string();
    let segment_url = sibling_url(&playback, &segment);
    let fetched = ivs.fetch(&segment_url).await;
    assert_eq!(
        fetched.status, 200,
        "every segment the playlist lists must be served, got {} {:?}",
        fetched.status, fetched.body
    );
    assert!(
        fetched.body.starts_with("FLV"),
        "a segment carries the ingested media, got {:?}",
        &fetched.body[..fetched.body.len().min(16)]
    );

    // And once the broadcast is over there is no playlist again, because nothing is live.
    let stopped = ivs.call("StopStream", &json!({ "channelArn": arn })).await;
    assert_eq!(
        stopped.status, 200,
        "StopStream must be accepted while the publisher runs, got {} {}",
        stopped.status, stopped.body
    );
    let after = ivs.fetch(&playback).await;
    assert_eq!(
        after.status, 404,
        "a channel that is not broadcasting serves no playlist, got {} {:?}",
        after.status, after.body
    );
    drop(publisher);
}

/// RULE, the unwind leg: `StopStream` disconnects the publisher of a live channel — the
/// channel reports `ChannelNotBroadcasting` again while that publisher is still sending,
/// its connection is severed, and the session the broadcast left behind reports both the
/// instant the channel went live and the instant it went offline.
#[tokio::test]
async fn stopping_a_stream_disconnects_its_publisher_and_closes_the_session() {
    let ivs = Ivs::new();
    let created = ivs.create(json!({ "name": "stopped" })).await;
    let (ingest, key) = contribute_target(&ivs, &created);
    let arn = text(&created["channel"], "arn");

    let mut publisher = Publisher::publishing(&ingest, &key)
        .await
        .unwrap_or_else(|error| panic!("the announced address must accept a publish: {error}"));
    publisher
        .send_configuration()
        .await
        .expect("the decoder configuration is sent");
    publisher.send_media(0, 2).await.expect("the media is sent");
    poll_until("a live channel", "GetStream to report the channel LIVE", || {
        let ivs = &ivs;
        let arn = arn.clone();
        async move { get_stream(ivs, &arn).await.status == 200 }
    })
    .await;
    let live = get_stream(&ivs, &arn).await;
    let stream_id = text(&live.body["stream"], "streamId");

    // Stopped while the publisher is still sending, which is the case AWS documents for
    // this operation.
    let stopped = ivs.call("StopStream", &json!({ "channelArn": arn })).await;
    assert_eq!(
        stopped.status, 200,
        "StopStream must be accepted while the publisher runs, got {} {}",
        stopped.status, stopped.body
    );
    let after = get_stream(&ivs, &arn).await;
    assert_not_broadcasting("after StopStream", &after, &arn);

    // The publisher's own connection is severed: an ingest session that was disconnected
    // cannot keep publishing, so the server closes it under the publisher rather than
    // taking a second broadcast from it.
    publisher
        .send_media(2, 1)
        .await
        .expect("a disconnected publisher may still write to its socket");
    assert!(
        severed(&mut publisher.conn).await,
        "a publisher whose stream was stopped must have its connection severed"
    );
    let still_offline = get_stream(&ivs, &arn).await;
    assert_not_broadcasting("while the stopped publisher keeps sending", &still_offline, &arn);

    // The session the broadcast left behind reports both instants, and is still reported
    // for the channel after the broadcast ended.
    let session = ivs
        .call(
            "GetStreamSession",
            &json!({ "channelArn": arn, "streamId": stream_id }),
        )
        .await;
    assert_eq!(
        session.status, 200,
        "GetStreamSession must report the ended broadcast, got {} {}",
        session.status, session.body
    );
    let reported = &session.body["streamSession"];
    assert_eq!(
        text(reported, "streamId"),
        stream_id,
        "the session is the broadcast the request named, got {reported}"
    );
    assert!(
        reported.get("startTime").is_some(),
        "an ended session reports when the channel went live, got {reported}"
    );
    assert!(
        reported.get("endTime").is_some(),
        "an ended session reports when the channel went offline, got {reported}"
    );
    assert_eq!(
        text(&reported["channel"], "arn"),
        arn,
        "a session carries the channel it belongs to, got {reported}"
    );

    // A stream id this deployment DID issue — the one minted for another channel's
    // broadcast — names no session of this channel, which is the resource that does not
    // exist. The whole class is the table in
    // `get_stream_session_judges_the_stream_id_before_the_channel`.
    let elsewhere = ivs.create(json!({ "name": "another-broadcast" })).await;
    let other_arn = text(&elsewhere["channel"], "arn");
    let other_stream = start_ingest(&ivs, &other_arn);
    let unknown = ivs
        .call(
            "GetStreamSession",
            &json!({ "channelArn": arn, "streamId": other_stream }),
        )
        .await;
    assert_eq!(
        unknown.status, 404,
        "a stream id this channel never had names no session of it, got {} {}",
        unknown.status, unknown.body
    );
}

/// RULE: `GetStreamSession` judges the `streamId` the request names BEFORE it resolves the
/// channel. A stream id is a value this service ISSUED — an opaque token it mints, never one
/// a caller composes — so a `streamId` no stream or session of this deployment carries fails
/// the INPUT (`ValidationException`, 400, naming the member and the constraint), while an id
/// it did issue that names no session of the channel asked about is a resource that does not
/// exist (`ResourceNotFoundException`, 404, naming that channel's ARN). An id of the channel
/// itself, and the absence of any id on a channel that has a session, answer the session.
///
/// The rows differ along what the rule generalises over: whether the value is an id this
/// deployment issued, which channel issued it, the alphabet and the case the value is
/// written in, whether the channel exists, and whether the request names an id at all.
#[tokio::test]
async fn get_stream_session_judges_the_stream_id_before_the_channel() {
    let ivs = Ivs::new();
    // Two channels a publisher has really contributed to, so each holds the session the
    // broadcast left behind, and one that nothing has published to.
    let broadcast = ivs.create(json!({ "name": "sessions-held" })).await;
    let broadcasting = text(&broadcast["channel"], "arn");
    let live_id = publish_session(&ivs, &broadcast);
    let elsewhere = ivs.create(json!({ "name": "sessions-elsewhere" })).await;
    let other_id = publish_session(&ivs, &elsewhere);
    let idle = ivs.create_arn(json!({ "name": "sessions-none" })).await;
    let absent = format!("{CHANNEL_ARN_PREFIX}neverCreated");

    // (case, the channel the request names, the streamId it names, the status and the code
    // the service must answer)
    let rows: Vec<(&str, &str, Option<String>, u16, &str)> = vec![
        (
            "the channel's own broadcast",
            &broadcasting,
            Some(live_id.clone()),
            200,
            "",
        ),
        (
            "no id at all on a channel that has a session",
            &broadcasting,
            None,
            200,
            "",
        ),
        (
            "an id this deployment issued, for another channel",
            &broadcasting,
            Some(other_id.clone()),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an id this deployment issued, on a channel that has no session",
            &idle,
            Some(live_id.clone()),
            404,
            "ResourceNotFoundException",
        ),
        (
            "no id at all on a channel that has no session",
            &idle,
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "an id this deployment issued, on a channel that was never created",
            &absent,
            Some(live_id.clone()),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a value of zeros nothing ever issued",
            &broadcasting,
            Some(unissued_stream_id("00000000000000000000000")),
            400,
            "ValidationException",
        ),
        (
            "a value in the hex alphabet this service's ids use, but never issued",
            &broadcasting,
            Some(unissued_stream_id("0123456789abcdef0123456")),
            400,
            "ValidationException",
        ),
        (
            "a value outside that alphabet",
            &broadcasting,
            Some(unissued_stream_id("notARealStreamIdValue01")),
            400,
            "ValidationException",
        ),
        (
            "the channel's own id in another case",
            &broadcasting,
            Some(live_id.to_ascii_uppercase().replace("ST-", "st-")),
            400,
            "ValidationException",
        ),
        (
            "a value nothing issued, on a channel that was never created: the id first",
            &absent,
            Some(unissued_stream_id("0000000000000000000beef")),
            400,
            "ValidationException",
        ),
        (
            "a value shorter than the modelled StreamId, refused by the constraint walk",
            &broadcasting,
            Some("st-short".to_string()),
            400,
            "ValidationException",
        ),
    ];

    for (case, channel_arn, stream_id, status, code) in rows {
        let request = match &stream_id {
            Some(id) => json!({ "channelArn": channel_arn, "streamId": id }),
            None => json!({ "channelArn": channel_arn }),
        };
        let answer = ivs.call("GetStreamSession", &request).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} {}",
            answer.status, answer.body
        );
        if status == 200 {
            let reported = text(&answer.body["streamSession"], "streamId");
            let expected = stream_id.clone().unwrap_or_else(|| live_id.clone());
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the session {expected:?}, got {}",
                answer.body
            );
            continue;
        }
        assert_eq!(
            text(&answer.body, "Code"),
            code,
            "case {case:?}: expected the error code {code:?}, got {}",
            answer.body
        );
        let message = text(&answer.body, "Message");
        let expected_message = match code {
            "ValidationException" => INVALID_STREAM_ID.to_string(),
            _ => format!("Resource: {channel_arn} not found"),
        };
        match (code, stream_id.as_deref()) {
            // The constraint walk words a MODELLED violation from the operation and the
            // member path, which is a different sentence of the same error class; what this
            // row pins is that the value is refused at all, with that class.
            ("ValidationException", Some(value)) if !stream_id_is_well_formed(value) => assert!(
                message.contains("streamId"),
                "case {case:?}: expected the refusal to name the member, got {message:?}"
            ),
            _ => assert_eq!(
                message, expected_message,
                "case {case:?}: expected {expected_message:?}, got {message:?}"
            ),
        }
    }
}

/// The session a real publish to the channel `created` names leaves behind: the broadcast
/// is admitted the way the contribute plane admits one, with that channel's own stream key,
/// so the id the rows use is an id this deployment really ISSUED.
fn publish_session(ivs: &Ivs, created: &Value) -> String {
    let key = text(&created["streamKey"], "value");
    ivs.with_store_mut(|store| state::accept_publish(store, &key))
        .expect("the channel's own stream key admits a publish")
        .stream_id
}

/// The service's one sentence for a `streamId` that is not a stream id it issued: the
/// member-constraint sentence, naming the member the id travelled in and the constraint it
/// failed.
///
/// Shared with `error_envelope.rs`, which pins the ENVELOPE that sentence travels in, so the
/// two suites cannot disagree about the text.
pub(crate) const INVALID_STREAM_ID: &str =
    "Request member: streamId failed to satisfy the constraints: invalid stream id";

/// A value satisfying the modelled `StreamId` ([`stream_id_is_well_formed`]) that no stream
/// of this emulator carries, so the constraint walk admits it and the handler is what judges
/// it.
///
/// Shared with `error_envelope.rs` for the same reason as [`INVALID_STREAM_ID`].
pub(crate) fn unissued_stream_id(body: &str) -> String {
    let id = format!("st-{body}");
    assert!(
        stream_id_is_well_formed(&id),
        "case {id:?}: a row's unissued id must still satisfy the modelled StreamId"
    );
    id
}

/// RULE: an admitted publish may only send what the plane could serve — a message larger
/// than one segment of the live window may hold is packaged nowhere, so its publisher is
/// disconnected instead of growing this store with media nothing keeps.
///
/// Rows differ along what the rule generalises over: the size of one ingested message, at
/// the published bound and one byte past it. What an accepted message costs is bounded by
/// the live window itself (`playback_plane`'s own suite covers that window).
#[tokio::test]
async fn a_message_larger_than_a_segment_disconnects_its_publisher() {
    // The bound itself, read from where it is stated rather than copied: the most media
    // bytes one packaged segment holds, and therefore the most one ingested message may
    // carry.
    let cap = state::MAX_SEGMENT_BYTES;

    let ivs = Ivs::new();
    let created = ivs.create(json!({ "name": "bounded-ingest" })).await;
    let arn = text(&created["channel"], "arn");
    let key = text(&created["streamKey"], "value");
    let publish = ivs
        .with_store_mut(|store| state::accept_publish(store, &key))
        .expect("the channel's own stream key admits a publish");

    let rows: [(&str, usize, bool); 3] = [
        ("one byte", 1, true),
        ("a message filling a whole segment", cap, true),
        ("one byte past a whole segment", cap + 1, false),
    ];
    for (case, size, served) in rows {
        let payload = vec![0x27_u8; size];
        let keeps_serving = ivs.with_store_mut(|store| {
            state::publish_media(store, &publish, state::MediaKind::Video, 0, &payload)
        });
        assert_eq!(
            keeps_serving,
            served,
            "case {case:?}: expected the publisher to be {}, got {keeps_serving}",
            match served {
                true => "served",
                false => "disconnected",
            }
        );
        // The refusal is the publisher's alone: the broadcast it brought up is untouched, so
        // the channel is still live and still reports the stream the publish started.
        let reply = get_stream(&ivs, &arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the channel stays live while its publisher is connected, got \
             {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            text(&reply.body["stream"], "streamId"),
            publish.stream_id,
            "case {case:?}: the channel reports the stream the publish started, got {}",
            reply.body["stream"]
        );
    }
}

/// RULE, the whole live path a broadcaster drives from what the create announced: an encoder
/// that dials `channel.ingestEndpoint` as it stands and authenticates with
/// `streamKey.value` brings that channel live, metadata is accepted while it is live,
/// `StopStream` takes it offline again, and `GetStream` answers `ChannelNotBroadcasting`
/// before the publish and after the stop.
///
/// Rows differ along the WIRE the encoder opens on that one announced address — `rtmps://`,
/// which is TLS from the first byte and what AWS documents an encoder to use, and `rtmp://`
/// in the clear — because the member carries no scheme and the publisher chooses.
#[tokio::test]
async fn the_announced_address_serves_the_whole_live_path_on_both_wires() {
    let ivs = Ivs::new();
    let wires: [(&str, bool); 2] = [
        ("rtmps, TLS from the first byte", true),
        ("rtmp, in the clear", false),
    ];
    for (case, encrypted) in wires {
        let created = ivs
            .create(json!({ "name": "announced-wire", "latencyMode": "NORMAL" }))
            .await;
        let arn = text(&created["channel"], "arn");
        // Everything the broadcaster holds: the announced address and the minted key.
        let (ingest, key) = contribute_target(&ivs, &created);

        // Before the publish the channel is not broadcasting, with the modelled refusal.
        let offline = get_stream(&ivs, &arn).await;
        assert_not_broadcasting(&format!("{case}: before the publish"), &offline, &arn);

        let mut publisher = match encrypted {
            true => Publisher::publishing_over_tls(&ingest, &key).await,
            false => Publisher::publishing(&ingest, &key).await,
        }
        .unwrap_or_else(|error| panic!("case {case:?}: {ingest} must accept a publish: {error}"));
        publisher
            .send_configuration()
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: {error}"));
        publisher
            .send_media(0, 2)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: {error}"));

        // The publish is what brings the channel live, so the case waits for THAT condition.
        poll_until(case, "GetStream to report the channel LIVE", || {
            let ivs = &ivs;
            let arn = arn.clone();
            async move {
                let reply = get_stream(ivs, &arn).await;
                reply.status == 200 && text(&reply.body["stream"], "state") == "LIVE"
            }
        })
        .await;
        let live = get_stream(&ivs, &arn).await;
        assert_live_stream(
            case,
            &live,
            &created["channel"],
            &text(&live.body["stream"], "streamId"),
        );

        // Metadata a broadcaster inserts into the live stream is accepted, with the empty
        // body the model gives that operation.
        let metadata = ivs
            .call(
                "PutMetadata",
                &json!({ "channelArn": arn, "metadata": "{\"event\":\"goal\"}" }),
            )
            .await;
        assert_eq!(
            metadata.status, 204,
            "case {case:?}: PutMetadata on a live channel must answer HTTP 204, got {} {}",
            metadata.status, metadata.body
        );

        // And the stop unwinds it: the channel is not broadcasting again, with the same
        // modelled refusal it answered before the publish.
        let stopped = ivs.call("StopStream", &json!({ "channelArn": arn })).await;
        assert_eq!(
            stopped.status, 200,
            "case {case:?}: StopStream must be accepted while the publisher runs, got {} {}",
            stopped.status, stopped.body
        );
        let after = get_stream(&ivs, &arn).await;
        assert_not_broadcasting(&format!("{case}: after StopStream"), &after, &arn);
    }
}

/// A URL beside `url`: the same address and directory, with `name` as the file — which is
/// how a player resolves the relative URI a playlist names.
fn sibling_url(url: &str, name: &str) -> String {
    let cut = url.rfind('/').map_or(url.len(), |end| end + 1);
    format!("{}{}", &url[..cut], name.trim())
}
