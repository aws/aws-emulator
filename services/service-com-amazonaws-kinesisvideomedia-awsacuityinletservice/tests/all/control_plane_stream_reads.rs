// SPDX-License-Identifier: CC0-1.0

//! `GetMedia` over the streams the Kinesis Video Streams CONTROL plane created.
//!
//! Every case drives one router carrying both emulated services over ONE shared engine,
//! so a stream created through `kinesisvideo` `CreateStream` is the very record this
//! data plane resolves — through the same request walk (route -> deserialize -> validate
//! -> handle -> serialize) a socket-served request takes.
//!
//! The rule under test: one Kinesis video stream is ONE resource, owned by the control
//! plane, so `GetMedia` reads the stream that service's registry holds under the
//! request's `StreamName` or `StreamARN` — a registered stream is readable whatever
//! media it holds (an empty one answers an empty payload), the one ARN `CreateStream`
//! minted addresses it on both planes, and `ResourceNotFoundException` is only ever for
//! an identifier that registry does not hold.
//!
//! The tables span the amount of media produced (none, one chunk, several), both
//! identifier forms, and the start selectors that read from either end of a log; the
//! request the issue recorded is one row.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as control_plane;
use service_com_amazonaws_kinesisvideomedia_awsacuityinletservice as media;
use smithy_server_runtime::MultiServiceRouter;

use media::{ComAmazonawsKinesisvideomediaAwsacuityinletserviceState, EmulatedService};

use crate::get_media::split_payload;

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The host a `GetDataEndpoint` answer points a data-plane call at: this emulator,
/// never a real AWS host.
const ENDPOINT_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// Producer timestamp of the first produced fragment, in epoch milliseconds.
const BASE_MILLIS: u64 = 1_761_000_000_000;

/// Milliseconds between two consecutive produced fragments.
const FRAGMENT_SPACING_MILLIS: u64 = 2_000;

/// The media bytes this suite produces, longest log first in the table rows below:
/// different lengths and contents, so a payload that concatenated them wrongly cannot
/// pass.
const MEDIA: &[&[u8]] = &[b"first-chunk-bytes", b"2", b"\x00\x01\xff-third-chunk"];

fn at(millis: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(millis)
}

/// One router over one engine, carrying the media data plane AND the Kinesis Video
/// Streams control plane — what makes a created stream readable here.
///
/// `pub(crate)` because the refusal-wording suite beside this one
/// (`refusal_message_fidelity`) drives the same two services over the same seams, and one
/// harness is what keeps the two suites agreeing on how a stream is created, deleted and
/// read.
pub(crate) struct Harness {
    pub(crate) router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(control_plane::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// POST one restJson1 call at the data endpoint host. The credential scope's
    /// service field is the routing key; no authenticator is registered, so the
    /// signature itself is not checked.
    pub(crate) async fn call(&self, path: &str, body: &serde_json::Value) -> http::Response<Bytes> {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{ENDPOINT_HOST}{path}"))
            .header("host", ENDPOINT_HOST)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{REGION}/kinesisvideo/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.router.dispatch(request).await
    }

    /// Create `name` through the control plane and answer the `StreamARN` it minted.
    pub(crate) async fn create_stream(&self, name: &str) -> String {
        let response = self
            .call(
                "/createStream",
                &serde_json::json!({ "StreamName": name, "DataRetentionInHours": 1 }),
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "case \"create {name}\": expected HTTP 200, got {} ({})",
            status(&response),
            error_code(&response)
        );
        let body = json(&response);
        body.get("StreamARN")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case \"create {name}\": no StreamARN in {body:?}"))
            .to_string()
    }

    /// The `StreamInfo` the control plane reports for `name`.
    async fn describe_stream(&self, name: &str) -> serde_json::Value {
        let response = self
            .call("/describeStream", &serde_json::json!({ "StreamName": name }))
            .await;
        assert_eq!(
            status(&response),
            200,
            "case \"describe {name}\": expected HTTP 200, got {} ({})",
            status(&response),
            error_code(&response)
        );
        json(&response)
            .get("StreamInfo")
            .cloned()
            .unwrap_or_else(|| panic!("case \"describe {name}\": no StreamInfo"))
    }

    /// Wait until the control plane reports `name` as `ACTIVE` — the status its own
    /// clock owes a freshly created stream, and the one `DeleteStream` requires.
    pub(crate) async fn wait_until_active(&self, name: &str) {
        for _ in 0..100 {
            let info = self.describe_stream(name).await;
            if info.get("Status").and_then(serde_json::Value::as_str) == Some("ACTIVE") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("case \"activate {name}\": the stream never reached ACTIVE");
    }

    /// The `DataEndpoint` a `GetDataEndpoint` call answers for reading `name`.
    async fn data_endpoint(&self, name: &str) -> String {
        let response = self
            .call(
                "/getDataEndpoint",
                &serde_json::json!({ "StreamName": name, "APIName": "GET_MEDIA" }),
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "case \"getDataEndpoint {name}\": expected HTTP 200, got {} ({})",
            status(&response),
            error_code(&response)
        );
        let body = json(&response);
        body.get("DataEndpoint")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case \"getDataEndpoint {name}\": no DataEndpoint in {body:?}"))
            .to_string()
    }

    /// Produce `chunks` to the stream the given identifier names, through this
    /// service's producer entry point on the shared engine (there is no `PutMedia`
    /// operation yet), and answer the fragment numbers it stored them under.
    pub(crate) fn produce(
        &self,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
        chunks: &[&[u8]],
    ) -> Vec<String> {
        let state = media::state::service_state(&*self.engine).expect("the media store");
        let mut store = state.lock();
        chunks
            .iter()
            .enumerate()
            .map(|(index, chunk)| {
                let millis = BASE_MILLIS + (index as u64) * FRAGMENT_SPACING_MILLIS;
                store
                    .put_fragment(
                        &*self.engine,
                        stream_name,
                        stream_arn,
                        chunk.to_vec(),
                        at(millis),
                        at(millis + 100),
                    )
                    .unwrap_or_else(|| {
                        panic!(
                            "case \"produce to {stream_name:?}/{stream_arn:?}\": \
                             a registered stream must accept media"
                        )
                    })
            })
            .collect()
    }

    /// `GetMedia` for the given addressing members and selector.
    pub(crate) async fn get_media(
        &self,
        addressing: &serde_json::Value,
        selector: serde_json::Value,
    ) -> http::Response<Bytes> {
        let mut body = addressing.as_object().cloned().unwrap_or_default();
        body.insert("StartSelector".to_string(), selector);
        self.call("/getMedia", &serde_json::Value::Object(body)).await
    }

    /// Bring forward the removal the control plane already scheduled for `name`.
    ///
    /// `DeleteStream` "marks the stream for deletion" and the control plane owes the
    /// record's removal at an instant IT fixed; this only moves that instant to now, so
    /// the test reads the state a caller arriving after the span would have seen,
    /// without sleeping through it.
    pub(crate) fn expire_deletion(&self, name: &str) {
        let state = control_plane::state::service_state(&*self.engine)
            .expect("the control plane store");
        let mut store = state.lock();
        let stream = store
            .streams
            .get_mut(name)
            .unwrap_or_else(|| panic!("case \"expire {name}\": the stream is still registered"));
        assert!(
            stream.deletes_at.is_some(),
            "case \"expire {name}\": the delete must have scheduled the removal"
        );
        stream.deletes_at = Some(Instant::now());
    }
}

pub(crate) fn status(response: &http::Response<Bytes>) -> u16 {
    response.status().as_u16()
}

pub(crate) fn error_code(response: &http::Response<Bytes>) -> String {
    response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn json(response: &http::Response<Bytes>) -> serde_json::Value {
    serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
}

fn content_type(response: &http::Response<Bytes>) -> String {
    response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// RULE: a stream the control plane created is readable on the media data plane,
/// whichever identifier form addresses it and whatever media it holds — a created
/// stream nothing was produced to answers an empty payload, not a missing resource.
#[tokio::test]
async fn a_stream_the_control_plane_created_is_readable_on_the_media_plane() {
    let harness = Harness::new();

    // Rows differ along HOW MUCH media was produced to the stream: none at all (the
    // case the issue recorded), one chunk, and a log of several.
    for produced in [0usize, 1, MEDIA.len()] {
        let name = format!("probe-produced-{produced}");
        let arn = harness.create_stream(&name).await;

        // The data plane a reader is told to use is resolved for this same stream, and it
        // is an address of THIS emulator — a loopback authority with the port its listener
        // was bound on — rather than a public AWS name: a reader dials exactly the value
        // it is handed. (The owning crate's own suite is what dials it.)
        let endpoint = harness.data_endpoint(&name).await;
        let announced_scheme = format!("{}://", control_plane::state::DATA_PLANE_SCHEME);
        let authority = endpoint.strip_prefix(&announced_scheme).unwrap_or_else(|| {
            panic!(
                "case \"produced {produced}\": expected a {announced_scheme} origin, \
                 got {endpoint:?}"
            )
        });
        let (host, port) = authority.rsplit_once(':').unwrap_or_else(|| {
            panic!(
                "case \"produced {produced}\": expected the announced endpoint to carry its \
                 port, got {endpoint:?}"
            )
        });
        assert!(
            ["127.0.0.1", "::1", "[::1]", "localhost"].contains(&host)
                && port.parse::<u16>().is_ok(),
            "case \"produced {produced}\": expected a data endpoint of this machine, \
             got {endpoint:?}"
        );

        let chunks = &MEDIA[..produced];
        let numbers = harness.produce(Some(&name), None, chunks);
        assert_eq!(
            numbers.len(),
            produced,
            "case \"produced {produced}\": expected {produced} stored fragments, \
             got {numbers:?}"
        );

        for (form, addressing) in [
            ("StreamName", serde_json::json!({ "StreamName": name })),
            ("StreamARN", serde_json::json!({ "StreamARN": arn })),
        ] {
            // EARLIEST reads the whole log, NOW only its newest chunk; an empty log
            // has neither, which is an empty payload under both.
            for (selector_type, expected) in [
                ("EARLIEST", chunks),
                ("NOW", &chunks[produced.saturating_sub(1)..]),
            ] {
                let case = format!("produced {produced} / {form} / {selector_type}");
                let response = harness
                    .get_media(
                        &addressing,
                        serde_json::json!({ "StartSelectorType": selector_type }),
                    )
                    .await;
                assert_eq!(
                    status(&response),
                    200,
                    "case {case:?}: expected HTTP 200, got {} ({}) {:?}",
                    status(&response),
                    error_code(&response),
                    json(&response)
                );
                assert_eq!(
                    content_type(&response),
                    "video/webm",
                    "case {case:?}: expected the stream's content type, got {:?}",
                    content_type(&response)
                );
                assert_eq!(
                    response.body().is_empty(),
                    expected.is_empty(),
                    "case {case:?}: expected {} payload, got {} byte(s)",
                    if expected.is_empty() { "an empty" } else { "a non-empty" },
                    response.body().len()
                );
                let delivered: Vec<String> = split_payload(&case, response.body(), expected)
                    .iter()
                    .map(|chunk| chunk.tag("AWS_KINESISVIDEO_FRAGMENT_NUMBER"))
                    .collect();
                let want: Vec<String> = numbers[numbers.len() - expected.len()..].to_vec();
                assert_eq!(
                    delivered, want,
                    "case {case:?}: expected fragments {want:?}, got {delivered:?}"
                );
            }
        }
    }
}

/// RULE: the one `StreamARN` `CreateStream` minted is the stream's identity on BOTH
/// planes — the control plane reports it, and the data plane resolves the same stream
/// through it as through the name, reading one log either way.
#[tokio::test]
async fn one_stream_identity_serves_the_control_plane_and_the_data_plane() {
    let harness = Harness::new();
    let name = "probe-one-identity";
    let created_arn = harness.create_stream(name).await;

    let reported_arn = harness
        .describe_stream(name)
        .await
        .get("StreamARN")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        reported_arn, created_arn,
        "case \"one ARN\": DescribeStream must report the ARN CreateStream minted, \
         got {reported_arn:?}"
    );

    // Produced through the ARN form, read back through the name form: one stream.
    let numbers = harness.produce(None, Some(created_arn.as_str()), MEDIA);
    let by_name = harness
        .get_media(
            &serde_json::json!({ "StreamName": name }),
            serde_json::json!({ "StartSelectorType": "EARLIEST" }),
        )
        .await;
    let by_arn = harness
        .get_media(
            &serde_json::json!({ "StreamARN": created_arn }),
            serde_json::json!({ "StartSelectorType": "EARLIEST" }),
        )
        .await;
    assert_eq!(
        status(&by_name),
        200,
        "case \"read by name\": expected HTTP 200, got {} ({})",
        status(&by_name),
        error_code(&by_name)
    );
    assert_eq!(
        by_name.body(),
        by_arn.body(),
        "case \"one identity\": the two identifier forms must answer the same media"
    );
    let delivered: Vec<String> = split_payload("one identity", by_name.body(), MEDIA)
        .iter()
        .map(|chunk| chunk.tag("AWS_KINESISVIDEO_FRAGMENT_NUMBER"))
        .collect();
    assert_eq!(
        delivered, numbers,
        "case \"one identity\": expected the produced fragments {numbers:?}, \
         got {delivered:?}"
    );

    // And exactly one media log exists for the stream, held under the ARN the control
    // plane minted: the data plane keeps no second copy of the stream.
    let logs: Vec<String> = {
        let state = harness
            .engine
            .state::<ComAmazonawsKinesisvideomediaAwsacuityinletserviceState>();
        let store = state.lock();
        let mut keys: Vec<String> = store.streams.keys().cloned().collect();
        keys.sort();
        keys
    };
    assert_eq!(
        logs,
        vec![created_arn.clone()],
        "case \"one log\": expected exactly the registered stream's log, got {logs:?}"
    );
}

/// RULE: a stream is the record the control plane holds, not its name — so a stream
/// created again under a removed stream's name is a DIFFERENT stream, with its own
/// `StreamARN` and no media of its own, and the removed stream's cursors do not resume
/// on it.
#[tokio::test]
async fn a_recreated_stream_does_not_inherit_the_removed_streams_media() {
    let harness = Harness::new();
    let name = "probe-recreated";

    let first_arn = harness.create_stream(name).await;
    harness.produce(Some(name), None, MEDIA);
    let read = harness
        .get_media(
            &serde_json::json!({ "StreamName": name }),
            serde_json::json!({ "StartSelectorType": "EARLIEST" }),
        )
        .await;
    assert_eq!(
        status(&read),
        200,
        "case \"first incarnation\": expected HTTP 200, got {} ({})",
        status(&read),
        error_code(&read)
    );
    let token = split_payload("first incarnation", read.body(), MEDIA)
        .first()
        .map(|chunk| chunk.tag("AWS_KINESISVIDEO_CONTINUATION_TOKEN"))
        .unwrap_or_default();

    harness.wait_until_active(name).await;
    let deleted = harness
        .call("/deleteStream", &serde_json::json!({ "StreamARN": first_arn }))
        .await;
    assert_eq!(
        status(&deleted),
        200,
        "case \"delete\": expected HTTP 200, got {} ({})",
        status(&deleted),
        error_code(&deleted)
    );
    harness.expire_deletion(name);

    let second_arn = harness.create_stream(name).await;
    assert_ne!(
        second_arn, first_arn,
        "case \"recreate\": the new stream must carry its own StreamARN, got {second_arn:?}"
    );

    // (case, selector, expected status, expected error code)
    let rows: Vec<(&str, serde_json::Value, u16, &str)> = vec![
        (
            "the recreated stream from the earliest chunk",
            serde_json::json!({ "StartSelectorType": "EARLIEST" }),
            200,
            "",
        ),
        (
            "the recreated stream from its latest chunk",
            serde_json::json!({ "StartSelectorType": "NOW" }),
            200,
            "",
        ),
        (
            "a cursor minted for the removed stream",
            serde_json::json!({
                "StartSelectorType": "CONTINUATION_TOKEN",
                "ContinuationToken": token,
            }),
            400,
            "InvalidArgumentException",
        ),
    ];

    for (case, selector, expected_status, expected_code) in rows {
        let response = harness
            .get_media(&serde_json::json!({ "StreamName": name }), selector)
            .await;
        assert_eq!(
            status(&response),
            expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} ({}) {:?}",
            status(&response),
            error_code(&response),
            json(&response)
        );
        if expected_code.is_empty() {
            assert!(
                response.body().is_empty(),
                "case {case:?}: the recreated stream holds no media, so the payload must \
                 be empty, got {} byte(s)",
                response.body().len()
            );
        } else {
            assert_eq!(
                error_code(&response),
                expected_code,
                "case {case:?}: expected {expected_code}, got {:?}",
                error_code(&response)
            );
        }
    }
}

/// RULE: `ResourceNotFoundException` (HTTP 404) is for an identifier the control-plane
/// registry does not hold, and for nothing else — including a stream it no longer
/// holds, whether or not media was produced to it.
#[tokio::test]
async fn a_missing_resource_is_only_a_stream_the_registry_does_not_hold() {
    let harness = Harness::new();

    // A live stream, to prove the refusal rows are about the registry and not about
    // this suite's addressing.
    let live_arn = harness.create_stream("probe-live").await;
    harness.produce(Some("probe-live"), None, &MEDIA[..1]);

    // Two deleted streams: one that held media and one that never did. A delete needs
    // the stream ACTIVE, which the control plane's own clock owes it.
    for (name, produced) in [("probe-deleted-empty", 0usize), ("probe-deleted-full", 2)] {
        let arn = harness.create_stream(name).await;
        harness.produce(Some(name), None, &MEDIA[..produced]);
        harness.wait_until_active(name).await;
        let response = harness
            .call("/deleteStream", &serde_json::json!({ "StreamARN": arn }))
            .await;
        assert_eq!(
            status(&response),
            200,
            "case \"delete {name}\": expected HTTP 200, got {} ({}) {:?}",
            status(&response),
            error_code(&response),
            json(&response)
        );
        harness.expire_deletion(name);
    }

    let unknown_arn = live_arn.replace("probe-live", "probe-never-created");
    // (case, addressing, expected status, expected error code)
    let rows: Vec<(&str, serde_json::Value, u16, &str)> = vec![
        (
            "a stream the registry holds",
            serde_json::json!({ "StreamName": "probe-live" }),
            200,
            "",
        ),
        (
            "a name nothing created",
            serde_json::json!({ "StreamName": "probe-never-created" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a well-formed ARN the registry never held",
            serde_json::json!({ "StreamARN": unknown_arn }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a deleted stream that held no media",
            serde_json::json!({ "StreamName": "probe-deleted-empty" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a deleted stream that held media",
            serde_json::json!({ "StreamName": "probe-deleted-full" }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, addressing, expected_status, expected_code) in rows {
        let response = harness
            .get_media(
                &addressing,
                serde_json::json!({ "StartSelectorType": "EARLIEST" }),
            )
            .await;
        assert_eq!(
            status(&response),
            expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} ({}) {:?}",
            status(&response),
            error_code(&response),
            json(&response)
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&response),
                expected_code,
                "case {case:?}: expected {expected_code}, got {:?}",
                error_code(&response)
            );
        }
    }
}
