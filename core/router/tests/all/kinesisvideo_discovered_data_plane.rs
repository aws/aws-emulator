// SPDX-License-Identifier: CC0-1.0

//! The data plane `GetDataEndpoint` DISCOVERS is one this emulator serves, and the media
//! reads of that plane are bound on it.
//!
//! The rule under test: **an endpoint an operation hands back for a client to dial is an
//! authority this emulator answers on, with that plane's routes bound there**.
//! `GetDataEndpoint` is the only documented way to find a stream's media planes ("You must
//! first call the GetDataEndpoint API to get an endpoint. Then send the ListFragments
//! requests to this endpoint using the --endpoint-url parameter", `ListFragments`), and the
//! client dials exactly the host it is handed, so an announced address this process does not
//! answer on makes the whole producer/consumer path unreachable however well-formed the
//! value looks.
//!
//! Three crates have to agree for that to hold — the Kinesis Video Streams control plane
//! announces the address, Kinesis Video Media serves `/getMedia` on it and Kinesis Video
//! Archived Media serves `/listFragments` — so the suite lives here. That the announced
//! address is a socket of THIS process, and that what answers there is the emulator's own
//! front door, is pinned in the control plane's own suite
//! (`tests/all/stream_data_endpoint.rs` of the kinesisvideo crate); what this suite adds is
//! the other half: the media and archived-media operations of the plane are served when a
//! call is addressed to the announced authority, exactly as when it is addressed to the
//! endpoint the control call arrived on.
//!
//! The rows differ along what the rule generalises over: which `APIName` was asked for (so
//! which plane's address is announced), which media operation of that plane is then called,
//! and whether the call is addressed to the announced authority or to the control-plane one
//! — where a route bound on the announced address alone is answered by the front door
//! instead.

use std::net::{Ipv4Addr, Ipv6Addr, TcpStream};
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call here is signed for.
const REGION: &str = "us-west-2";

/// The control-plane address the calls that discover the data plane arrive on — a bare
/// loopback listener, the form a client on this machine reaches the emulator at.
const CONTROL_AUTHORITY: &str = "127.0.0.1:9297";

/// The stream the suite discovers the planes of.
const STREAM: &str = "discovered-plane-probe";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as the router mounts them, with request authentication off so each call carries a
/// credential scope (which is what names the Region and the service) without a computed
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

/// What one call answered: its status and its parsed JSON body.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The error code the envelope names, or `""` for a body that names none.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// POST one restJson1 call to `authority`, signed for `kinesisvideo` in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    authority: &str,
    path: &str,
    body: &Value,
) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{authority}{path}"))
        .header("host", authority)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/kinesisvideo/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// The `DataEndpoint` the control plane announces for `api_name`, asserting the 200.
async fn data_endpoint(router: &MultiServiceRouter, api_name: &str) -> String {
    let answer = call(
        router,
        CONTROL_AUTHORITY,
        "/getDataEndpoint",
        &json!({ "StreamName": STREAM, "APIName": api_name }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {api_name:?}: GetDataEndpoint expected HTTP 200, got {answer:?}"
    );
    answer
        .body
        .get("DataEndpoint")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {api_name:?}: no DataEndpoint in {answer:?}"))
        .to_string()
}

/// The authority of an announced origin, failing the case when the value carries a path:
/// "The returned endpoint does not have the API name appended. The client needs to add the
/// API name to the returned endpoint."
///
/// The scheme is the one the plane's listener speaks, which the control plane publishes as
/// `DATA_PLANE_SCHEME`: the announced origin has to be one the calling client dials as it
/// stands.
fn announced_authority(case: &str, endpoint: &str) -> String {
    let scheme = format!(
        "{}://",
        service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::state::DATA_PLANE_SCHEME
    );
    let authority = endpoint.strip_prefix(&scheme).unwrap_or_else(|| {
        panic!("case {case:?}: expected a {scheme} origin, got {endpoint:?}")
    });
    assert!(
        !authority.is_empty()
            && !authority.contains('/')
            && !authority.contains('?')
            && !authority.contains('#'),
        "case {case:?}: expected scheme and host only with no path, got {endpoint:?}"
    );
    authority.to_string()
}

/// A connection to an announced endpoint: what the value exists for, so an address this
/// emulator does not answer on fails here.
///
/// The announced host has to be one of this machine's own loopback addresses — the
/// interface the emulator's core binds its listeners on — and the dial is to that address
/// and no other.
fn assert_dialable(case: &str, authority: &str) {
    let (host, port) = authority.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: expected the announced endpoint to carry its port, got {authority:?}")
    });
    let port = port
        .parse::<u16>()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a port, got {authority:?}"));
    let dialed = match host.trim_start_matches('[').trim_end_matches(']') {
        "127.0.0.1" => TcpStream::connect((Ipv4Addr::LOCALHOST, port)),
        "::1" => TcpStream::connect((Ipv6Addr::LOCALHOST, port)),
        "localhost" => TcpStream::connect(("localhost", port)),
        other => panic!(
            "case {case:?}: the announced endpoint must be an address of this machine, got \
             {other:?} in {authority:?}"
        ),
    };
    assert!(
        dialed.is_ok(),
        "case {case:?}: the announced endpoint {authority:?} must accept a connection, got {:?}",
        dialed.err()
    );
}

/// A fragment selector covering the whole archive, which `ListFragments` requires on a first
/// page ("This is only required when the NextToken isn't passed in the API").
fn whole_archive() -> Value {
    json!({
        "FragmentSelectorType": "SERVER_TIMESTAMP",
        "TimestampRange": { "StartTimestamp": 0, "EndTimestamp": 4_000_000_000_i64 },
    })
}

/// Wait until the control plane reports `stream` as `ACTIVE`, which is what a client does
/// before reading a stream it just created ("CreateStream is an asynchronous operation"),
/// and what the archived-media plane serves reads for.
///
/// Waits for the CONDITION, not for a span: it looks again until the status holds, under a
/// deadline far longer than the transition needs.
async fn await_active(router: &MultiServiceRouter, stream: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        let answer = call(
            router,
            CONTROL_AUTHORITY,
            "/describeStream",
            &json!({ "StreamName": stream }),
        )
        .await;
        last = answer.body["StreamInfo"]["Status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if last == "ACTIVE" {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("DescribeStream expected Status ACTIVE before the deadline, got {last:?}");
}

/// The discovered data plane is reachable, and the media reads of that plane are served when
/// a call is addressed to the authority the control plane announced.
#[tokio::test(flavor = "multi_thread")]
async fn the_announced_data_endpoint_serves_the_media_reads_of_its_plane() {
    let router = emulator();
    let created = call(
        &router,
        CONTROL_AUTHORITY,
        "/createStream",
        &json!({ "StreamName": STREAM, "MediaType": "video/h264" }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateStream expected HTTP 200, got {created:?}"
    );
    await_active(&router, STREAM).await;

    // One row per media endpoint the published `APIName` set is partitioned into: the live
    // media endpoint, which binds the ingest and the live read, and the archived-media one.
    let write = data_endpoint(&router, "PUT_MEDIA").await;
    let read = data_endpoint(&router, "GET_MEDIA").await;
    let write_authority = announced_authority("PUT_MEDIA", &write);
    let read_authority = announced_authority("GET_MEDIA", &read);
    for (case, authority) in [("PUT_MEDIA", &write_authority), ("GET_MEDIA", &read_authority)] {
        assert_ne!(
            authority, CONTROL_AUTHORITY,
            "case {case:?}: the data authority must not be the control-plane one, got \
             {authority:?}"
        );
        assert_dialable(case, authority);
    }
    assert_eq!(
        write_authority, read_authority,
        "the ingest and the live read are two APIs of one media endpoint, so one authority \
         serves both, got {write_authority:?} and {read_authority:?}"
    );
    // Every API the ARCHIVED-media endpoint binds shares that endpoint's one address, which
    // is not the live media endpoint's.
    let archive = data_endpoint(&router, "LIST_FRAGMENTS").await;
    let archive_authority = announced_authority("LIST_FRAGMENTS", &archive);
    assert_ne!(
        archive_authority, read_authority,
        "the endpoint that reads stored fragments must not be announced at the live media \
         one's address, got {archive_authority:?} for both"
    );
    assert_dialable("LIST_FRAGMENTS", &archive_authority);
    for api_name in ["GET_MEDIA_FOR_FRAGMENT_LIST", "GET_CLIP"] {
        let endpoint = data_endpoint(&router, api_name).await;
        assert_eq!(
            endpoint, archive,
            "case {api_name:?}: every API the archived-media endpoint binds resolves to one \
             endpoint, got {endpoint:?}"
        );
    }

    // The media and archived-media operations, each addressed to the authority the control
    // plane announced for the endpoint that binds it: each is served by the service that
    // models it. The same call addressed to the CONTROL-plane authority meets the other half
    // of the split — a stream's media APIs are not served at the endpoint the discovery call
    // is made to ("You must first call the GetDataEndpoint API to get an endpoint. Then send
    // the GetMedia requests to this endpoint"), so Kinesis Video Media's own route is bound
    // on the announced address alone and the control-plane front door answers it as the
    // unroutable request it is. Archived Media's `/listFragments` is that service's route
    // and is not scoped by this emulator yet, so it still answers at either address; what
    // the row pins is that the announced authority is an address of THIS emulator rather
    // than a different one.
    //
    // (case, path, the authority announced for the endpoint binding it, the status there,
    // the status at the control-plane authority)
    let reads: &[(&str, &str, &String, u16, u16, Value)] = &[
        (
            "GetMedia on the announced authority",
            "/getMedia",
            &read_authority,
            200,
            400,
            json!({ "StreamName": STREAM, "StartSelector": { "StartSelectorType": "EARLIEST" } }),
        ),
        (
            "ListFragments on the announced authority",
            "/listFragments",
            &archive_authority,
            200,
            200,
            json!({ "StreamName": STREAM, "FragmentSelector": whole_archive() }),
        ),
    ];
    for (case, path, announced, on_the_plane, on_the_control_plane, body) in reads {
        for (authority, expected) in [
            (announced.as_str(), *on_the_plane),
            (CONTROL_AUTHORITY, *on_the_control_plane),
        ] {
            let answer = call(&router, authority, path, body).await;
            assert_eq!(
                answer.status, expected,
                "case {case:?} at {authority:?}: expected HTTP {expected} from the service that \
                 binds the route there, got {answer:?}"
            );
            if expected == 200 {
                assert_eq!(
                    answer.error_code(),
                    "",
                    "case {case:?} at {authority:?}: expected no refusal, got {answer:?}"
                );
            } else {
                // The front door's own answer to a request it can name no operation for:
                // the fault travels in the envelope's `Output`, so no `__type` sits at the
                // body's root for `error_code` to read.
                assert_eq!(
                    answer.body["Output"]["__type"].as_str(),
                    Some("com.amazon.kinesis.video.v20170930#UnknownOperationException"),
                    "case {case:?} at {authority:?}: expected the front door's \
                     unknown-operation envelope, got {answer:?}"
                );
            }
        }
    }

    // And the stream the reads are of is the one the control plane holds: the planes are
    // addresses of one emulator over one store, so the control plane still answers for it.
    let described = call(
        &router,
        CONTROL_AUTHORITY,
        "/describeStream",
        &json!({ "StreamName": STREAM }),
    )
    .await;
    assert_eq!(
        described.status, 200,
        "DescribeStream expected HTTP 200, got {described:?}"
    );
    assert_eq!(
        described.body["StreamInfo"]["StreamName"].as_str(),
        Some(STREAM),
        "DescribeStream must report the stream the planes were discovered for, got {described:?}"
    );
}
