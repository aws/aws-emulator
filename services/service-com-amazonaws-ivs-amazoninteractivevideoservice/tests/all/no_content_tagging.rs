// SPDX-License-Identifier: CC0-1.0

//! What this emulator answers for a call that leaves its caller NO CONTENT, and which
//! resources its tagging surface can address, driven over the wire.
//!
//! Every case goes through the real front door — `MultiServiceRouter::dispatch` with a
//! restJson1 request, exactly as a signed `aws ivs` call arrives — so what it asserts is
//! the emulator's HTTP behaviour, not a handler called directly.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A tagging WRITE — `POST /tags/{resourceArn}` and `DELETE /tags/{resourceArn}`,
//!    whose modelled outputs are structures with no member — is answered `204 No
//!    Content`, and every answer this service sends at that status carries no body and
//!    states the length it has (`content-length: 0`). An answer that carries content is
//!    untouched.
//! 2. A resource's tags are readable and writable through the tagging surface by the
//!    resource's own ARN, for EVERY resource type this service can create — so a tag set
//!    written when the resource was created is readable afterwards whatever kind of
//!    resource carries it.
//!
//! The rows of rule 2 are the six resource types this service's stores hold (a channel,
//! a stream key, a playback key pair, an ad configuration, a recording configuration and
//! a failover slate configuration), one created with tags and one without. Rule 1's rows
//! are the two tagging writes over each of those resources, plus each delete whose
//! modelled `@http` code is 204 (`smithy_query(query="digest")`); `PutMetadata`, the
//! remaining modelled 204, is answered through the same one serialization seam.

use std::collections::HashMap;
use std::sync::Arc;

use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivs_amazoninteractivevideoservice::EmulatedService;

/// A SigV4 `Authorization` header is how a request names the service and the region it
/// is addressed to, so every call carries one. The signature itself is never checked
/// here: request authentication is off unless a router turns it on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The status an answer that carries no content is sent at, and the length such an
/// answer states for the body it does not carry.
const NO_CONTENT: u16 = 204;
const NO_CONTENT_LENGTH: &str = "0";

/// One IVS emulator over one shared engine, reached the way the router reaches it.
///
/// Each test suite under `tests/all/` is its own private module, so this is the
/// per-suite copy of that two-line harness rather than a shared one.
struct Ivs {
    router: MultiServiceRouter,
}

/// One answer from the emulator, down to the framing: the status, the `content-length`
/// it states, the raw body bytes and the parsed body.
#[derive(Debug)]
struct Reply {
    status: u16,
    content_length: Option<String>,
    bytes: Vec<u8>,
    body: Value,
}

impl Ivs {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivs.us-east-1.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", AUTHORIZATION)
            .body(
                body.map(|body| bytes::Bytes::from(body.to_string()))
                    .unwrap_or_default(),
            )
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_length = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.body().to_vec();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Reply {
            status,
            content_length,
            bytes,
            body,
        }
    }

    /// An operation bound to `POST /<Operation>`, which is every IVS operation except
    /// the three tagging ones.
    async fn post(&self, operation: &str, body: Value) -> Reply {
        self.send("POST", &format!("/{operation}"), Some(body)).await
    }

    /// Call `operation` and require it to have succeeded, returning the body.
    async fn ok(&self, operation: &str, body: Value) -> Value {
        let reply = self.post(operation, body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {operation} {body}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body
    }

    /// One of the three operations bound to `/tags/{resourceArn}`, which carry the ARN in
    /// the URI the way a client does: percent-encoded into one path segment.
    async fn tagging(&self, method: &str, arn: &str, query: &str, body: Option<Value>) -> Reply {
        let path = format!("/tags/{}{query}", encode_path_label(arn));
        self.send(method, &path, body).await
    }

    /// The tags the surface answers for `arn`, which it must answer with HTTP 200.
    async fn tags_of(&self, case: &str, arn: &str) -> HashMap<String, String> {
        let reply = self.tagging("GET", arn, "", None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListTagsForResource on {arn} must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        serde_json::from_value(reply.body["tags"].clone()).unwrap_or_default()
    }
}

/// Percent-encode one `@httpLabel` value, the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment. Every character
/// an IVS ARN can hold is ASCII, so one byte per character.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// A string member, read as the empty string when absent — which is what a client sees
/// for a member the wire omits.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A tag map as a `HashMap`, for comparing what a read answers with what was written.
fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A freshly generated public key as a `PUBLIC KEY` PEM block, which is what
/// `ImportPlaybackKeyPair` takes: generated rather than hard-coded, so what is imported
/// is a real public key.
fn public_key_pem() -> String {
    let der = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .expect("the test key pair is generated")
        .public_key_der();
    pem::encode(&pem::Pem::new("PUBLIC KEY", der))
}

/// One row of rule 2: a resource of one type, its ARN, and the tags its own create
/// persisted on it.
struct Taggable {
    case: &'static str,
    arn: String,
    at_creation: HashMap<String, String>,
}

/// Create one resource of every type this service can create, and return the rows.
///
/// Each ARN comes from the create that minted the resource, never from a literal, so a
/// row addresses a resource this emulator really holds.
async fn taggable_resources(ivs: &Ivs) -> Vec<Taggable> {
    let created = ivs
        .ok(
            "CreateChannel",
            json!({ "name": "tagged-channel", "tags": { "probe": "channel" } }),
        )
        .await;
    let channel_arn = text(&created["channel"], "arn");
    // The stream key `CreateChannel` mints with the channel carries no tags of its own,
    // which is the row where the surface answers a tag set the create did NOT write.
    let stream_key_arn = text(&created["streamKey"], "arn");

    let key_pair = ivs
        .ok(
            "ImportPlaybackKeyPair",
            json!({ "name": "tagged-key", "publicKeyMaterial": public_key_pem(),
                    "tags": { "probe": "playback-key" } }),
        )
        .await;
    let key_pair_arn = text(&key_pair["keyPair"], "arn");

    // An entry carrying no `playbackConfigurationArn` references no MediaTailor playback
    // configuration, so this create resolves nothing; the suite that drives the
    // reference itself is `referenced_resource_admission.rs`.
    let ad_configuration = ivs
        .ok(
            "CreateAdConfiguration",
            json!({ "name": "tagged-ads", "mediaTailorPlaybackConfigurations": [ {} ],
                    "tags": { "probe": "ad-configuration" } }),
        )
        .await;
    let ad_configuration_arn = text(&ad_configuration["adConfiguration"], "arn");

    // No object store is mounted in this suite, so nothing ANSWERS what the destination
    // names and the create is refused nothing — the one-sided reading
    // `referenced_resource_admission.rs` states.
    let recording_configuration = ivs
        .ok(
            "CreateRecordingConfiguration",
            json!({ "name": "tagged-recording",
                    "destinationConfiguration": { "s3": { "bucketName": "ivs-recordings" } },
                    "tags": { "probe": "recording-configuration" } }),
        )
        .await;
    let recording_configuration_arn =
        text(&recording_configuration["recordingConfiguration"], "arn");

    let failover_slate = ivs
        .ok(
            "CreateFailoverSlateConfiguration",
            json!({ "name": "tagged-slate",
                    "tags": { "probe": "failover-slate-configuration" } }),
        )
        .await;
    let failover_slate_arn = text(&failover_slate["failoverSlateConfiguration"], "arn");

    vec![
        Taggable {
            case: "a channel, tagged at creation",
            arn: channel_arn,
            at_creation: tags(&[("probe", "channel")]),
        },
        Taggable {
            case: "a stream key, created without tags",
            arn: stream_key_arn,
            at_creation: HashMap::new(),
        },
        Taggable {
            case: "a playback key pair, tagged at import",
            arn: key_pair_arn,
            at_creation: tags(&[("probe", "playback-key")]),
        },
        Taggable {
            case: "an ad configuration, tagged at creation",
            arn: ad_configuration_arn,
            at_creation: tags(&[("probe", "ad-configuration")]),
        },
        Taggable {
            case: "a recording configuration, tagged at creation",
            arn: recording_configuration_arn,
            at_creation: tags(&[("probe", "recording-configuration")]),
        },
        Taggable {
            case: "a failover slate configuration, tagged at creation",
            arn: failover_slate_arn,
            at_creation: tags(&[("probe", "failover-slate-configuration")]),
        },
    ]
}

/// Assert `reply` is an answer that carries no content: the no-content status, no body
/// byte at all, and the length it states for it.
fn assert_carries_no_content(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, NO_CONTENT,
        "case {case:?}: expected HTTP {NO_CONTENT}, got {} {}",
        reply.status, reply.body
    );
    assert!(
        reply.bytes.is_empty(),
        "case {case:?}: a no-content answer carries no body, got {:?}",
        String::from_utf8_lossy(&reply.bytes)
    );
    assert_eq!(
        reply.content_length.as_deref(),
        Some(NO_CONTENT_LENGTH),
        "case {case:?}: it states the length it has, got {:?}",
        reply.content_length
    );
}

/// Rule 2 and rule 1's tagging half together: for a resource of every type this service
/// can create, the surface answers the tags its create persisted, a write adds and
/// replaces them, a removal takes the named keys away — and each write is answered with
/// no content.
#[tokio::test]
async fn the_tagging_surface_addresses_every_resource_type_and_writes_answer_no_content() {
    let ivs = Ivs::new();
    for Taggable {
        case,
        arn,
        at_creation,
    } in taggable_resources(&ivs).await
    {
        assert_eq!(
            ivs.tags_of(case, &arn).await,
            at_creation,
            "case {case:?}: the surface must answer the tags the create persisted for {arn}"
        );

        // A tag added, and one of the same key replaced.
        let written = ivs
            .tagging(
                "POST",
                &arn,
                "",
                Some(json!({ "tags": { "team": "video", "probe": "rewritten" } })),
            )
            .await;
        assert_carries_no_content(&format!("TagResource on {case}"), &written);
        assert_eq!(
            ivs.tags_of(case, &arn).await,
            tags(&[("team", "video"), ("probe", "rewritten")]),
            "case {case:?}: a written key takes the new value and every other tag is kept"
        );

        // A removal, and the same removal again: `UntagResource` is modelled
        // `@idempotent`, so repeating it answers the same way and leaves the same tags.
        for round in 0..2 {
            let removed = ivs.tagging("DELETE", &arn, "?tagKeys=team", None).await;
            assert_carries_no_content(&format!("UntagResource round {round} on {case}"), &removed);
            assert_eq!(
                ivs.tags_of(case, &arn).await,
                tags(&[("probe", "rewritten")]),
                "case {case:?} round {round}: the named key is gone and the rest is kept"
            );
        }
    }
}

/// Rule 1's other half: every operation whose modelled success code is 204 is answered
/// with the same framing, and an answer that CARRIES content keeps every byte of it.
#[tokio::test]
async fn every_answer_at_the_no_content_status_is_framed_the_same_way() {
    let ivs = Ivs::new();
    let rows = taggable_resources(&ivs).await;
    let arn_of = |case: &str| {
        rows.iter()
            .find(|row| row.case == case)
            .map(|row| row.arn.clone())
            .expect("the row this case names was created")
    };

    // Rows are the modelled 204s of this service, each named by the operation and the
    // resource it removes; the channel goes last because its delete takes its stream key
    // with it.
    let cases = vec![
        (
            "DeleteStreamKey",
            json!({ "arn": arn_of("a stream key, created without tags") }),
        ),
        (
            "DeleteAdConfiguration",
            json!({ "arn": arn_of("an ad configuration, tagged at creation") }),
        ),
        (
            "DeleteRecordingConfiguration",
            json!({ "arn": arn_of("a recording configuration, tagged at creation") }),
        ),
        (
            "DeleteFailoverSlateConfiguration",
            json!({ "arn": arn_of("a failover slate configuration, tagged at creation") }),
        ),
        (
            "DeleteChannel",
            json!({ "arn": arn_of("a channel, tagged at creation") }),
        ),
    ];
    for (operation, request) in cases {
        let reply = ivs.post(operation, request.clone()).await;
        assert_carries_no_content(&format!("{operation} {request}"), &reply);
    }

    // The control row: a read answers content, so its body is the renderer's and no
    // framing is applied to it.
    let listed = ivs.post("ListChannels", json!({})).await;
    assert_eq!(
        listed.status, 200,
        "case ListChannels after the deletes: expected HTTP 200, got {} {}",
        listed.status, listed.body
    );
    assert!(
        !listed.bytes.is_empty(),
        "case ListChannels after the deletes: an answer that carries content keeps its body"
    );
}

/// A PROBE of what a real socket answers for a tagging write, beside what this service
/// states above.
///
/// The framing a service states is not the last word on the wire: the HTTP/1.1 writer
/// this emulator is served by removes `content-length` from a `204` (RFC 9110 §15.3.5
/// forbids a body there), and that writer is a dependency of the vendored HTTP layer
/// rather than anything a service crate decides — the sibling `ivsrealtime` emulator
/// records the same boundary in its own `wire_probe.rs`. So what is pinned here is what
/// the socket really delivers: the status, the absence of a body, and that the answer is
/// not chunked.
#[tokio::test(flavor = "multi_thread")]
async fn a_tagging_write_on_the_wire_carries_the_status_and_no_body() {
    let ivs = Ivs::new();
    let created = ivs
        .ok("CreateChannel", json!({ "name": "wire-probe" }))
        .await;
    let arn = text(&created["channel"], "arn");

    let (endpoint, _served) = emulator_server::serve_on_ephemeral_port(ivs.router.clone()).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));

    let path = format!("/tags/{}", encode_path_label(&arn));
    let answer = tokio::task::spawn_blocking(move || raw_tag_request(port, &path))
        .await
        .expect("the socket exchange runs to completion");

    assert!(
        answer.starts_with(&format!("HTTP/1.1 {NO_CONTENT}")),
        "case TagResource on the wire: expected a {NO_CONTENT} answer, got {answer:?}"
    );
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("a response head and body, got {answer:?}"));
    assert!(
        body.is_empty(),
        "case TagResource on the wire: a no-content answer carries no body, got {body:?}"
    );
    assert!(
        !head.to_ascii_lowercase().contains("transfer-encoding"),
        "case TagResource on the wire: an empty answer is not chunked, got {head:?}"
    );
    // The boundary itself, pinned the way the `dataexchange` emulator pins it: the
    // `content-length: 0` the service states (asserted over the in-process front door
    // above) does not reach a socket client, because the writer below this crate removes
    // it from a no-content answer.
    assert!(
        !head.to_ascii_lowercase().contains("content-length"),
        "case TagResource on the wire: the HTTP/1.1 writer below this crate drops \
         content-length from a no-content answer, got {head:?}"
    );
}

/// One `TagResource` request written straight onto a socket, with everything the server
/// answered read back as text.
///
/// `connection: close` so the server closes the socket after answering and the read below
/// terminates on EOF rather than on a parse of the framing this very case is about.
fn raw_tag_request(port: u16, path: &str) -> String {
    use std::io::{Read, Write};

    let payload = json!({ "tags": { "probe": "wire" } }).to_string();
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         host: localhost\r\n\
         authorization: {AUTHORIZATION}\r\n\
         content-type: application/json\r\n\
         content-length: {}\r\n\
         connection: close\r\n\
         \r\n{payload}",
        payload.len()
    );
    stream
        .write_all(request.as_bytes())
        .expect("write the request");
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .expect("read the whole answer");
    String::from_utf8_lossy(&answer).to_string()
}
