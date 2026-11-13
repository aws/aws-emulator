// SPDX-License-Identifier: CC0-1.0

//! `GetDataEndpoint`: which endpoint a stream's media APIs are addressed at, and
//! whether that address is one this emulator ANSWERS on.
//!
//! The rule under test is that an endpoint an operation hands back for the client to
//! DIAL is an authority this emulator serves: `DataEndpoint` is "The endpoint value. To
//! read data from the stream or to write data to it, specify this endpoint in your
//! application", and nothing rewrites the host in between, so a value naming an address
//! this process does not answer on makes the discovered data plane unreachable however
//! well-formed it looks. The value is therefore a bare `scheme://host[:port]` origin —
//! no path, because "The returned endpoint does not have the API name appended. The
//! client needs to add the API name to the returned endpoint" — of the media endpoint
//! that serves the requested `APIName`, distinct per endpoint (Kinesis Video Media's
//! `InvalidEndpointException`, "Caller used wrong endpoint to write data to a stream …
//! the user must call `GetDataEndpoint` with `AccessMode` set to \"READ\"", is why an API
//! addressed at an endpoint that does not bind it is refused) and distinct from the
//! control-plane endpoint the call itself arrived on. Which operations each address SERVES is the rule
//! `crate::service::data_plane_address` and the media services' own route claims state,
//! asserted by `tests/all/data_plane_addresses.rs`.
//!
//! The concrete host and port are the environment's and nothing here asserts them. What
//! the cases assert is the shape of the value, that the address accepts a connection and
//! is answered by this emulator's own front door over the transport its scheme names, the
//! partition of the published `APIName` set it induces, and its independence from the
//! address the request came in on — over a table whose rows differ in the API asked for,
//! the stream named, the form the stream is named by, the region of the credential scope,
//! and the host the request arrives on (an AWS-form control-plane host and a bare loopback
//! listener).

use std::collections::{BTreeMap, BTreeSet};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as kvs;
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A router carrying only this service, over its own engine, with request
/// authentication off so a case can send an unsigned-but-scoped request.
fn harness() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(kvs::EmulatedService::new(engine));
    router
}

/// One control-plane address a case addresses the emulator at: the authority the
/// request arrives on, and the region of its SigV4 credential scope.
#[derive(Debug, Clone, Copy)]
struct ControlPlane {
    authority: &'static str,
    region: &'static str,
}

/// The control planes the table drives: the service's own regional endpoint, and a bare
/// loopback listener in another region — the two forms a request actually arrives on.
const CONTROL_PLANES: &[ControlPlane] = &[
    ControlPlane {
        authority: "kinesisvideo.us-east-1.amazonaws.com",
        region: "us-east-1",
    },
    ControlPlane {
        authority: "127.0.0.1:35573",
        region: "eu-west-2",
    },
];

/// POST one restJson1 call to `control` and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    control: ControlPlane,
    path: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let ControlPlane { authority, region } = control;
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{authority}{path}"))
        .header("host", authority)
        .header("content-type", "application/json")
        // The SigV4 credential scope is what routes the request to this service and
        // names its region; the signature itself is not checked here.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kinesisvideo/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// Create `name` on `control` and return its `StreamARN`.
async fn create_stream(router: &MultiServiceRouter, control: ControlPlane, name: &str) -> String {
    let (status, body) = call(
        router,
        control,
        "/createStream",
        serde_json::json!({ "StreamName": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateStream expected HTTP 200, got {status} with {body:?}"
    );
    body.get("StreamARN")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {name:?}: CreateStream returned no StreamARN: {body:?}"))
        .to_string()
}

/// `GetDataEndpoint` for one identifier and `APIName`, asserting the 200 and returning
/// the `DataEndpoint` value.
async fn data_endpoint(
    router: &MultiServiceRouter,
    control: ControlPlane,
    case: &str,
    identifier: serde_json::Value,
    api_name: &str,
) -> String {
    let mut request = identifier;
    request
        .as_object_mut()
        .expect("the identifier is a JSON object")
        .insert("APIName".to_string(), serde_json::json!(api_name));
    let (status, body) = call(router, control, "/getDataEndpoint", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body:?}"
    );
    body.get("DataEndpoint")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no DataEndpoint in {body:?}"))
        .to_string()
}

/// The authority of a `scheme://host` base URL, failing the case when the value carries
/// a path, a query or not the scheme the plane's address serves: "The returned endpoint
/// does not have the API name appended. The client needs to add the API name to the
/// returned endpoint", so anything after the host would be appended to rather than
/// replaced.
fn base_url_authority(case: &str, endpoint: &str) -> String {
    let scheme = format!("{}://", kvs::state::DATA_PLANE_SCHEME);
    let rest = endpoint.strip_prefix(&scheme).unwrap_or_else(|| {
        panic!("case {case:?}: expected a {scheme} base URL, got {endpoint:?}")
    });
    assert!(
        !rest.is_empty() && !rest.contains('/') && !rest.contains('?') && !rest.contains('#'),
        "case {case:?}: expected scheme and host only with no path, got {endpoint:?}"
    );
    rest.to_string()
}

/// The host and port of an announced authority, as a client dials them: an IPv6 literal
/// keeps its own colons inside the brackets the authority writes it in.
fn host_and_port(case: &str, authority: &str) -> (String, u16) {
    let (host, port) = match authority.strip_prefix('[') {
        Some(rest) => {
            let (host, tail) = rest.split_once(']').unwrap_or_else(|| {
                panic!("case {case:?}: expected a bracketed IPv6 host, got {authority:?}")
            });
            (host.to_string(), tail.trim_start_matches(':'))
        }
        None => {
            let (host, port) = authority.rsplit_once(':').unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an announced endpoint to carry the port it is \
                     served on, got {authority:?}"
                )
            });
            (host.to_string(), port)
        }
    };
    let port = port
        .parse::<u16>()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a port, got {authority:?}"));
    (host, port)
}

/// Open a TCP connection to an announced endpoint of THIS emulator.
///
/// The address is the one the operation handed back, so this dial is the client's own:
/// it is what the whole value exists for, and a value naming an address nothing listens
/// on fails here. The announced host has to be one of this machine's own loopback
/// addresses — the interface the emulator's core binds its listeners on — and the dial
/// below is to that address and no other.
async fn connect(case: &str, endpoint: &str) -> tokio::net::TcpStream {
    let authority = base_url_authority(case, endpoint);
    let (host, port) = host_and_port(case, &authority);
    let dialed = match host.as_str() {
        "127.0.0.1" => tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await,
        "::1" => tokio::net::TcpStream::connect((Ipv6Addr::LOCALHOST, port)).await,
        "localhost" => tokio::net::TcpStream::connect(("localhost", port)).await,
        other => panic!(
            "case {case:?}: the announced endpoint must be an address of this machine, got \
             {other:?} in {endpoint:?}"
        ),
    };
    dialed.unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the announced endpoint {endpoint:?} must accept a connection, \
             got {error}"
        )
    })
}

/// The answer a dialled endpoint gave: the status of the response, and its raw tail (the
/// bytes after the header block, which a close-delimited or chunked body both carry).
#[derive(Debug)]
struct DialedAnswer {
    status: u16,
    tail: String,
}

/// Send one HTTP/1.1 request to `endpoint` as a client that was handed that endpoint
/// does, and read the whole answer.
///
/// The request goes onto the connection as it stands, with nothing negotiated first: the
/// announced origin's scheme names the transport the client speaks, which is the whole
/// point of announcing it — a default-configured client has no trust material for this
/// emulator beyond what it was given for the endpoint the control call arrived on.
///
/// `Connection: close` is what makes the answer end at EOF, so no body framing has to be
/// re-implemented here: the tail is searched for what the case expects to find in it.
async fn send_to_endpoint(
    case: &str,
    endpoint: &str,
    path: &str,
    region: &str,
    body: serde_json::Value,
) -> DialedAnswer {
    let authority = base_url_authority(case, endpoint);
    let mut conn = connect(case, endpoint).await;
    let body = serde_json::to_vec(&body).expect("the request body serializes");
    let head = format!(
        "POST {path} HTTP/1.1\r\n\
         host: {authority}\r\n\
         content-type: application/json\r\n\
         authorization: AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kinesisvideo/\
         aws4_request, SignedHeaders=host, Signature=0\r\n\
         content-length: {}\r\n\
         connection: close\r\n\r\n",
        body.len()
    );
    conn.write_all(head.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: writing to {endpoint:?} failed: {error}"));
    conn.write_all(&body)
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: writing to {endpoint:?} failed: {error}"));
    conn.flush()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: writing to {endpoint:?} failed: {error}"));

    let mut answer = Vec::new();
    conn.read_to_end(&mut answer)
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: reading {endpoint:?} failed: {error}"));
    let answer = String::from_utf8_lossy(&answer).to_string();
    let (head, tail) = answer.split_once("\r\n\r\n").unwrap_or_else(|| {
        panic!("case {case:?}: expected an HTTP response from {endpoint:?}, got {answer:?}")
    });
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse::<u16>().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an HTTP status line from {endpoint:?}, got {head:?}")
        });
    DialedAnswer {
        status,
        tail: tail.to_string(),
    }
}

/// The API that WRITES media onto a stream, and the one the ingest route is addressed at.
/// From the `APIName` enum (`smithy_query(query="shape", shape="APIName")`).
const PUT_MEDIA: &str = "PUT_MEDIA";

/// Which media endpoint of this family serves each published `APIName`: the LIVE media
/// endpoint (Kinesis Video Media, whose model binds `GetMedia`, with the recorded
/// `PutMedia` ingest beside it) or the ARCHIVED-media one (Kinesis Video Archived Media,
/// whose model binds `GetClip`, `GetDASHStreamingSessionURL`, `GetHLSStreamingSessionURL`,
/// `GetImages`, `GetMediaForFragmentList` and `ListFragments`).
///
/// Written out here from each endpoint's own model (`smithy_query(query="digest")` on the
/// Kinesis Video Media and Kinesis Video Archived Media models) rather than read back off
/// the code under test, so a regrouping of the published API set has to fail here.
const SERVING_ENDPOINT: &[(&str, &str)] = &[
    ("PUT_MEDIA", "the live media endpoint"),
    ("GET_MEDIA", "the live media endpoint"),
    ("GET_CLIP", "the archived-media endpoint"),
    ("GET_DASH_STREAMING_SESSION_URL", "the archived-media endpoint"),
    ("GET_HLS_STREAMING_SESSION_URL", "the archived-media endpoint"),
    ("GET_IMAGES", "the archived-media endpoint"),
    ("GET_MEDIA_FOR_FRAGMENT_LIST", "the archived-media endpoint"),
    ("LIST_FRAGMENTS", "the archived-media endpoint"),
];

/// The endpoint the catalog says serves `api_name`.
fn serving_endpoint(api_name: &str) -> &'static str {
    SERVING_ENDPOINT
        .iter()
        .find(|(name, _)| *name == api_name)
        .map(|(_, endpoint)| *endpoint)
        .unwrap_or_else(|| panic!("case {api_name:?}: the catalog must name the endpoint serving it"))
}

/// Every `APIName` the model publishes resolves to the data endpoint of the media endpoint
/// that serves it: a bare base URL that this emulator accepts a connection on, never the
/// control-plane endpoint the call arrived on, with every API one media endpoint binds on
/// one address and the two endpoints on different ones.
#[tokio::test]
async fn each_api_name_resolves_to_a_reachable_endpoint_of_the_plane_that_serves_it() {
    // The catalog is about the published API set and nothing else: every value the model
    // publishes is placed, and nothing it does not.
    let catalogued: BTreeSet<&str> = SERVING_ENDPOINT.iter().map(|(name, _)| *name).collect();
    let published: BTreeSet<&str> = kvs::ops::types::A_P_I_NAME_VALUES.iter().copied().collect();
    assert_eq!(
        catalogued, published,
        "the catalog must name exactly the APIName values the model publishes, got \
         {catalogued:?} against {published:?}"
    );

    for control in CONTROL_PLANES {
        let control = *control;
        for stream in ["probe", "second.stream_2-B"] {
            let router = harness();
            let arn = create_stream(&router, control, stream).await;
            // The two identifier forms of one stream are the same resource, so they
            // name the same endpoint.
            let identifiers: Vec<(&str, serde_json::Value)> = vec![
                ("by name", serde_json::json!({ "StreamName": stream })),
                ("by ARN", serde_json::json!({ "StreamARN": arn })),
            ];

            let mut endpoints: BTreeMap<String, String> = BTreeMap::new();
            for api_name in kvs::ops::types::A_P_I_NAME_VALUES {
                for (form, identifier) in &identifiers {
                    let case = format!(
                        "{api_name} {form} on {} in {}",
                        control.authority, control.region
                    );
                    let endpoint =
                        data_endpoint(&router, control, &case, identifier.clone(), api_name).await;
                    let authority = base_url_authority(&case, &endpoint);

                    assert_ne!(
                        endpoint,
                        format!("{}://{}", kvs::state::DATA_PLANE_SCHEME, control.authority),
                        "case {case:?}: the data endpoint must not be the control-plane \
                         endpoint the call was made to, got {endpoint:?}"
                    );
                    assert_ne!(
                        authority, control.authority,
                        "case {case:?}: the data authority must not be the control-plane one, \
                         got {endpoint:?}"
                    );
                    // The API name is never appended to the value.
                    assert!(
                        !endpoint
                            .to_ascii_lowercase()
                            .contains(&api_name.to_ascii_lowercase().replace('_', "")),
                        "case {case:?}: the endpoint must not carry the API name, got \
                         {endpoint:?}"
                    );
                    // What the value is FOR: the client dials it. An address this
                    // emulator does not answer on fails right here.
                    drop(connect(&case, &endpoint).await);

                    // Both identifier forms of one stream name one endpoint.
                    let previous = endpoints.insert((*api_name).to_string(), endpoint.clone());
                    if let Some(previous) = previous {
                        assert_eq!(
                            previous, endpoint,
                            "case {case:?}: the two identifier forms of one stream must \
                             resolve to one endpoint, expected {previous:?}, got {endpoint:?}"
                        );
                    }
                }
            }

            // The published API set is partitioned by the media endpoint that serves each
            // API: the live media endpoint and the archived-media one. Each group is one
            // address, and one group's is not the other's.
            let mut grouped: BTreeMap<&str, BTreeSet<&String>> = BTreeMap::new();
            for (api_name, endpoint) in &endpoints {
                grouped
                    .entry(serving_endpoint(api_name))
                    .or_default()
                    .insert(endpoint);
            }
            assert_eq!(
                grouped.len(),
                2,
                "case {stream:?} on {}: the published APIs are served by two media \
                 endpoints, got {grouped:?}",
                control.authority
            );
            for (group, addresses) in &grouped {
                assert_eq!(
                    addresses.len(),
                    1,
                    "case {stream:?} {group:?} on {}: every API one media endpoint binds \
                     shares one data endpoint, got {endpoints:?}",
                    control.authority
                );
            }
            let distinct: BTreeSet<&&String> = grouped.values().flatten().collect();
            assert_eq!(
                distinct.len(),
                2,
                "case {stream:?} on {}: the two media endpoints are two addresses, \
                 got {endpoints:?}",
                control.authority
            );
        }
    }
}

/// Send one request to `endpoint` with the head written out verbatim and no body, and read
/// the whole answer — for the cases whose point is a header a well-behaved client does not
/// send.
async fn send_head_to_endpoint(case: &str, endpoint: &str, head: &str) -> DialedAnswer {
    let mut conn = connect(case, endpoint).await;
    conn.write_all(head.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: writing to {endpoint:?} failed: {error}"));
    conn.flush()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: writing to {endpoint:?} failed: {error}"));
    let mut answer = Vec::new();
    conn.read_to_end(&mut answer)
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: reading {endpoint:?} failed: {error}"));
    let answer = String::from_utf8_lossy(&answer).to_string();
    let (head, tail) = answer.split_once("\r\n\r\n").unwrap_or_else(|| {
        panic!("case {case:?}: expected an HTTP response from {endpoint:?}, got {answer:?}")
    });
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse::<u16>().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an HTTP status line from {endpoint:?}, got {head:?}")
        });
    DialedAnswer {
        status,
        tail: tail.to_string(),
    }
}

/// The transport-safety refusals an announced endpoint applies before anything of this
/// emulator is reached, which are the ones the core applies to its own HTTP listeners: a
/// request naming a host this emulator is not bound on (the guard a browser needs against
/// a rebound name), a request naming no host at all, a WebSocket upgrade, and a request a
/// browser sent from another site.
///
/// The rows differ in what the request carries, and every one of them is a request no
/// client of a data plane sends.
#[tokio::test]
async fn an_announced_endpoint_refuses_what_a_core_listener_refuses() {
    let control = CONTROL_PLANES[1];
    let router = harness();
    create_stream(&router, control, "guarded").await;
    let endpoint = data_endpoint(
        &router,
        control,
        "guarded",
        serde_json::json!({ "StreamName": "guarded" }),
        PUT_MEDIA,
    )
    .await;
    let authority = base_url_authority("guarded", &endpoint);

    // (case, the head sent, the status it earns).
    let cases: Vec<(&str, String, u16)> = vec![
        (
            "a host this emulator is not bound on",
            "POST /putMedia HTTP/1.1\r\nhost: kinesisvideo.us-east-1.amazonaws.com\r\n\
             content-length: 0\r\nconnection: close\r\n\r\n"
                .to_string(),
            421,
        ),
        (
            "no host at all",
            "POST /putMedia HTTP/1.0\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                .to_string(),
            421,
        ),
        (
            "a WebSocket upgrade",
            format!(
                "GET /putMedia HTTP/1.1\r\nhost: {authority}\r\nconnection: close, Upgrade\r\n\
                 upgrade: websocket\r\nsec-websocket-version: 13\r\n\r\n"
            ),
            501,
        ),
        (
            "a request a browser sent from another site",
            format!(
                "POST /putMedia HTTP/1.1\r\nhost: {authority}\r\norigin: https://evil.example\r\n\
                 sec-fetch-site: cross-site\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            ),
            403,
        ),
    ];

    for (case, head, expected) in &cases {
        let answer = send_head_to_endpoint(case, &endpoint, head).await;
        assert_eq!(
            answer.status, *expected,
            "case {case:?}: expected HTTP {expected} from the announced endpoint, got {answer:?}"
        );
    }
}

/// What answers at an announced endpoint is this emulator's OWN front door: a request
/// sent to the address the operation handed back is routed exactly as the same request
/// sent to the endpoint the control call arrived on, so the service that claims the
/// media path serves it — and in this harness, where only the control plane is
/// registered, the answer is the router's own refusal for a path no service of it
/// claims. Either way the answer comes from the emulator, which is what the address has
/// to be for.
#[tokio::test]
async fn an_announced_data_endpoint_is_answered_by_this_emulators_own_front_door() {
    let control = CONTROL_PLANES[1];
    let router = harness();
    create_stream(&router, control, "dialled").await;

    // One case per plane, each dialled with a media path that plane serves: the live
    // media endpoint's ingest path and the archived-media endpoint's listing path.
    let cases: &[(&str, &str)] = &[(PUT_MEDIA, "/putMedia"), ("LIST_FRAGMENTS", "/listFragments")];
    for (api_name, path) in cases {
        let case = format!("{api_name} dialled at {path}");
        let endpoint = data_endpoint(
            &router,
            control,
            &case,
            serde_json::json!({ "StreamName": "dialled" }),
            api_name,
        )
        .await;
        let answer = send_to_endpoint(
            &case,
            &endpoint,
            path,
            control.region,
            serde_json::json!({ "StreamName": "dialled" }),
        )
        .await;
        // The router's own refusal of a path no registered service claims: a 404 whose
        // envelope names `UnknownService`. Only this emulator's front door answers that,
        // so reading it back off the dialled connection is what pins the announced
        // address to the emulator rather than to some other listener.
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected this emulator's front door to answer the announced \
             endpoint, got {answer:?}"
        );
        assert!(
            answer.tail.contains("UnknownService"),
            "case {case:?}: expected the router's own refusal for a path no service of this \
             harness claims, got {answer:?}"
        );
    }
}

/// A connector that fails the way an emulator DEFECT does: by unwinding, rather than by
/// answering. It stands in for whatever the router would have dispatched a forwarded
/// request to.
#[derive(Debug)]
struct PanickingFrontDoor;

impl ::aws_smithy_runtime_api::client::http::HttpConnector for PanickingFrontDoor {
    fn call(
        &self,
        _request: ::aws_smithy_runtime_api::http::Request,
    ) -> ::aws_smithy_runtime_api::client::http::HttpConnectorFuture {
        panic!("an operation reached through the announced data endpoint panicked")
    }
}

/// A request that makes a handler behind an announced endpoint PANIC is answered as an
/// internal failure, not by dropping the caller's connection: the announced address
/// carries the same panic containment as the endpoint the control call arrived on, so one
/// request cannot take the listener — or the process serving every other caller — down
/// with it.
///
/// The rows differ in the request that reaches the panicking handler: each media path of
/// a plane, with and without a body, so the containment is not a property of one shape of
/// request.
#[tokio::test]
async fn a_panic_behind_an_announced_endpoint_is_contained_as_an_internal_failure() {
    let transport =
        ::aws_smithy_runtime_api::client::http::SharedHttpConnector::new(PanickingFrontDoor);
    let cases: &[(&str, &[u8])] = &[
        ("/putMedia", b"{\"StreamName\":\"dialled\"}"),
        ("/getMedia", b"{}"),
        ("/listFragments", b""),
    ];
    for (path, body) in cases {
        let request = http::Request::builder()
            .method("POST")
            .uri(*path)
            .header("host", "127.0.0.1:9297")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/kinesisvideo/\
                 aws4_request, SignedHeaders=host, Signature=0",
            )
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        let answer = kvs::state::serve_on_data_plane(transport.clone(), request).await;
        assert_eq!(
            answer.status().as_u16(),
            500,
            "case {path:?}: a contained panic is answered as an internal failure, got {:?}",
            answer.status()
        );
        assert!(
            answer
                .headers()
                .contains_key(::emulator_server::PANIC_CONTAINED_HEADER),
            "case {path:?}: expected the answer to be marked as a contained panic, got {:?}",
            answer.headers()
        );
    }
}

/// The endpoint is the PLANE's: stable however often it is read, shared by every stream
/// of the store that asks for that plane, and never the other plane's.
#[tokio::test]
async fn a_data_endpoint_is_stable_across_reads_and_is_the_planes_own() {
    let control = CONTROL_PLANES[0];
    let router = harness();
    let streams = ["alpha", "beta", "a.b_c-1"];
    for stream in streams {
        create_stream(&router, control, stream).await;
    }

    let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for api_name in [PUT_MEDIA, "GET_MEDIA", "LIST_FRAGMENTS"] {
        for stream in streams {
            let mut values = Vec::new();
            for round in 1..=3 {
                let case = format!("{stream} {api_name} read {round}");
                values.push(
                    data_endpoint(
                        &router,
                        control,
                        &case,
                        serde_json::json!({ "StreamName": stream }),
                        api_name,
                    )
                    .await,
                );
            }
            let first = values[0].clone();
            assert!(
                values.iter().all(|value| *value == first),
                "case {stream:?} {api_name:?}: reading the endpoint must not change it, \
                 got {values:?}"
            );
            seen.entry(api_name.to_string()).or_default().insert(first);
        }
    }

    for (api_name, values) in &seen {
        assert_eq!(
            values.len(),
            1,
            "case {api_name:?}: one plane is served at one address, whichever stream asks \
             for it, got {values:?}"
        );
    }
    // The partition is by the media endpoint that serves each API: the live media
    // endpoint `PUT_MEDIA` and `GET_MEDIA` share, and the archived-media one.
    let media = seen
        .get(PUT_MEDIA)
        .unwrap_or_else(|| panic!("no endpoint for {PUT_MEDIA}"));
    assert_eq!(
        seen.get("GET_MEDIA"),
        Some(media),
        "case \"GET_MEDIA\": the live read is served by the endpoint the ingest is, \
         got {seen:?}"
    );
    let archive = seen
        .get("LIST_FRAGMENTS")
        .unwrap_or_else(|| panic!("no endpoint for LIST_FRAGMENTS"));
    assert_ne!(
        archive, media,
        "case \"LIST_FRAGMENTS\": the endpoint that reads stored fragments is not the live \
         media one, got {seen:?}"
    );
}

/// Every value the model publishes is placed on a plane, and a value it does not
/// publish is placed on none — so no `APIName` is served an endpoint chosen for it, and
/// an unpublished one is refused as a bad argument.
#[tokio::test]
async fn every_published_api_name_is_placed_on_a_plane_and_nothing_else_is() {
    for api_name in kvs::ops::types::A_P_I_NAME_VALUES {
        assert!(
            kvs::state::data_plane_of(api_name).is_some(),
            "case {api_name:?}: every published APIName must resolve to a data plane"
        );
    }

    let control = CONTROL_PLANES[0];
    let router = harness();
    create_stream(&router, control, "placed").await;
    for case in ["NOT_AN_API", "put_media", "GET_MEDIA ", ""] {
        assert!(
            kvs::state::data_plane_of(case).is_none(),
            "case {case:?}: a value the model does not publish must resolve to no plane"
        );
        let (status, body) = call(
            &router,
            control,
            "/getDataEndpoint",
            serde_json::json!({ "StreamName": "placed", "APIName": case }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            body.get("__type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default(),
            // The boundary refuses an unpublished value before the handler runs, and the
            // stack that fronts a stream-addressed operation states such a refusal as the
            // generic validation one (this crate's `service::refusal_envelope`).
            "ValidationException",
            "case {case:?}: got {body:?}"
        );
    }
}
