// SPDX-License-Identifier: CC0-1.0

//! WHERE the modelled read route is served: the address the control plane announces for
//! the READ plane, and nowhere else on this emulator — and what an address that does not
//! bind it answers instead.
//!
//! The rule under test: a Kinesis Video Streams media API is bound on the address
//! `GetDataEndpoint` announces for the media endpoint that serves it — "You must first
//! call the `GetDataEndpoint` API to get an endpoint. Then send the `GetMedia` requests to
//! this endpoint using the `--endpoint-url` parameter", with this service's own
//! `InvalidEndpointException` stating the same rule from the data side — so `POST
//! /getMedia` is served at the origin announced for `GET_MEDIA`, which is this service's
//! own endpoint and so the origin `PUT_MEDIA` resolves to as well, and at no other
//! address of this emulator. An endpoint that binds no operation for a request's method
//! and URI answers it from the front door, ahead of any handler, in that family's
//! unknown-operation envelope: HTTP 400 with `Output.__type`, `Output.Message` and
//! `Version`, and nothing in it taken from the request.
//!
//! The first table's rows differ in the ADDRESS the identical call was sent to: the
//! announced live-media origin, the announced archived-media origin, three forms of this
//! emulator's own
//! front-door address (an IPv4 loopback authority, the `localhost` name and a bracketed
//! IPv6 literal), the service's published regional host — which this emulator announced no
//! plane at and cannot place on one, where the route's own claim stands — and a request
//! naming no host at all, which addresses nothing this emulator published either.
//!
//! The second table's rows differ in WHY the endpoint binds no operation for the request:
//! a method this service's model does not bind on its own route, asked for at the very
//! address that serves the read plane.

use std::sync::Arc;

use bytes::Bytes;
use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as control_plane;
use service_com_amazonaws_kinesisvideomedia_awsacuityinletservice as media;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for.
const REGION: &str = "us-east-1";

/// The service's published regional host — what a client that resolves AWS's own
/// hostnames addresses, rather than an address this emulator handed out.
const PUBLISHED_HOST: &str = "kinesisvideo.us-east-1.amazonaws.com";

/// The envelope the Kinesis Video Streams front door answers a request it can name no
/// operation for, written out here as the wire carries it rather than read back off the
/// code under test: the status is 400, the fault travels in `Output` with the front door's
/// own `__type` and its one sentence, and the envelope names its version.
pub(crate) fn unknown_operation_envelope() -> serde_json::Value {
    serde_json::json!({
        "Output": {
            "__type": "com.amazon.kinesis.video.v20170930#UnknownOperationException",
            "Message": "Unable to determine operation name",
        },
        "Version": "1.0",
    })
}

/// One router over one engine, carrying the control plane that creates the stream and
/// announces the planes, and this media service that serves the read route.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(control_plane::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(media::EmulatedService::new(engine));
        Self { router }
    }

    /// The SigV4 credential scope every request carries: its service field is the routing
    /// key the five Kinesis Video services share.
    fn authorization() -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261107/{REGION}/kinesisvideo/\
             aws4_request, SignedHeaders=host, Signature=0000"
        )
    }

    /// Send one restJson1 call with `method`, at `authority` when the row names one and
    /// with no host at all when it does not.
    async fn send(
        &self,
        method: &str,
        authority: Option<&str>,
        path: &str,
        body: &serde_json::Value,
    ) -> http::Response<Bytes> {
        let uri = match authority {
            Some(authority) => format!("https://{authority}{path}"),
            None => path.to_string(),
        };
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("authorization", Self::authorization());
        if let Some(authority) = authority {
            request = request.header("host", authority);
        }
        self.router
            .dispatch(
                request
                    .body(Bytes::from(
                        serde_json::to_vec(body).expect("the request body serializes"),
                    ))
                    .expect("a well-formed request"),
            )
            .await
    }

    /// POST one restJson1 call at `authority`.
    async fn call_at(
        &self,
        authority: &str,
        path: &str,
        body: &serde_json::Value,
    ) -> http::Response<Bytes> {
        self.send("POST", Some(authority), path, body).await
    }

    /// Create `name` through the control plane, at the address the discovery call is made
    /// at.
    async fn create_stream(&self, control: &str, name: &str) {
        let response = self
            .call_at(
                control,
                "/createStream",
                &serde_json::json!({ "StreamName": name, "DataRetentionInHours": 1 }),
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "case {name:?}: expected HTTP 200 from CreateStream, got {} {}",
            status(&response),
            text(&response)
        );
    }

    /// The authority `GetDataEndpoint` announces for `api_name`, with the scheme stripped
    /// off: the address a reader then dials.
    async fn announced_authority(&self, control: &str, name: &str, api_name: &str) -> String {
        let response = self
            .call_at(
                control,
                "/getDataEndpoint",
                &serde_json::json!({ "StreamName": name, "APIName": api_name }),
            )
            .await;
        assert_eq!(
            status(&response),
            200,
            "case {api_name:?}: expected HTTP 200 from GetDataEndpoint, got {} {}",
            status(&response),
            text(&response)
        );
        let endpoint = json(&response)
            .get("DataEndpoint")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let scheme = format!("{}://", control_plane::state::DATA_PLANE_SCHEME);
        endpoint
            .strip_prefix(&scheme)
            .unwrap_or_else(|| {
                panic!("case {api_name:?}: expected a {scheme} origin, got {endpoint:?}")
            })
            .to_string()
    }
}

fn status(response: &http::Response<Bytes>) -> u16 {
    response.status().as_u16()
}

fn text(response: &http::Response<Bytes>) -> String {
    String::from_utf8_lossy(response.body()).to_string()
}

fn json(response: &http::Response<Bytes>) -> serde_json::Value {
    serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null)
}

/// A `GetMedia` body reading one stream from the oldest chunk it holds.
fn read_from_the_start(name: &str) -> serde_json::Value {
    serde_json::json!({
        "StreamName": name,
        "StartSelector": { "StartSelectorType": "EARLIEST" },
    })
}

/// Which address a row sends the read request to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Address {
    /// The origin announced for `GET_MEDIA`, which `PUT_MEDIA` shares.
    AnnouncedReadPlane,
    /// The origin announced for the archived-media APIs.
    AnnouncedArchivePlane,
    /// A literal address of this emulator's own front door.
    FrontDoor(&'static str),
    /// The service's published regional host.
    PublishedHost,
    /// A request naming no host at all.
    NoHost,
}

/// One row: the address the identical request is sent to, and whether the read route is
/// served there.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    address: Address,
    served: bool,
}

/// The rows, differing along the address the reader sent the request to.
const ROWS: &[Row] = &[
    Row {
        case: "the announced live-media origin",
        address: Address::AnnouncedReadPlane,
        served: true,
    },
    Row {
        case: "the announced archived-media origin",
        address: Address::AnnouncedArchivePlane,
        served: false,
    },
    Row {
        case: "this emulator's front door, by its loopback address",
        address: Address::FrontDoor("127.0.0.1:9297"),
        served: false,
    },
    Row {
        case: "this emulator's front door, by name",
        address: Address::FrontDoor("localhost:9297"),
        served: false,
    },
    Row {
        case: "this emulator's front door, by its IPv6 literal",
        address: Address::FrontDoor("[::1]:9297"),
        served: false,
    },
    Row {
        case: "the service's published regional host",
        address: Address::PublishedHost,
        served: true,
    },
    Row {
        case: "a request naming no host at all",
        address: Address::NoHost,
        served: true,
    },
];

/// RULE: `POST /getMedia` is served at the origin announced for the media endpoint that
/// binds it — the one `GET_MEDIA` and `PUT_MEDIA` share; at every
/// other address of this emulator the path is bound to no operation, so the front door
/// answers it with HTTP 400 and the unknown-operation envelope, names no error type, and
/// reads no stream.
#[tokio::test]
async fn the_read_route_is_served_only_at_the_origin_announced_for_the_read_plane() {
    let harness = Harness::new();
    // The discovery calls are made at this emulator's own front door, as a client on this
    // machine makes them, and the planes are announced from there.
    let control = "127.0.0.1:9297";
    harness.create_stream(control, "read-probe").await;
    let read_plane = harness
        .announced_authority(control, "read-probe", "GET_MEDIA")
        .await;
    let write_plane = harness
        .announced_authority(control, "read-probe", "PUT_MEDIA")
        .await;
    let archive_plane = harness
        .announced_authority(control, "read-probe", "GET_MEDIA_FOR_FRAGMENT_LIST")
        .await;
    assert_eq!(
        read_plane, write_plane,
        "the live read and the ingest are one endpoint's APIs, so one origin serves both; \
         got {read_plane:?} against {write_plane:?}"
    );
    assert_ne!(
        read_plane, archive_plane,
        "the archived-media endpoint must be announced at its own address, \
         got {read_plane:?} for both"
    );
    assert_ne!(
        read_plane, control,
        "the read plane must not be announced at the control-plane address, got {read_plane:?}"
    );

    for row in ROWS {
        let Row {
            case,
            address,
            served,
        } = *row;
        let authority = match address {
            Address::AnnouncedReadPlane => Some(read_plane.clone()),
            Address::AnnouncedArchivePlane => Some(archive_plane.clone()),
            Address::FrontDoor(literal) => Some(literal.to_string()),
            Address::PublishedHost => Some(PUBLISHED_HOST.to_string()),
            Address::NoHost => None,
        };
        // The identical request, differing only in the address it is sent to.
        let response = harness
            .send(
                "POST",
                authority.as_deref(),
                "/getMedia",
                &read_from_the_start("read-probe"),
            )
            .await;

        if served {
            assert_eq!(
                status(&response),
                200,
                "case {case:?} ({authority:?}): expected the read route to be served, got {} {}",
                status(&response),
                text(&response)
            );
        } else {
            assert_eq!(
                status(&response),
                400,
                "case {case:?} ({authority:?}): expected the front door's unroutable-request \
                 answer, got {} {}",
                status(&response),
                text(&response)
            );
            assert_eq!(
                json(&response),
                unknown_operation_envelope(),
                "case {case:?} ({authority:?}): expected the unknown-operation envelope as the \
                 whole body, got {:?}",
                text(&response)
            );
            assert!(
                response.headers().get("x-amzn-errortype").is_none(),
                "case {case:?} ({authority:?}): no operation was identified, so none is named in \
                 the error-type header, got {:?}",
                response.headers().get("x-amzn-errortype")
            );
        }
    }
}

/// RULE: the same front-door answer serves every request an endpoint of this family binds
/// no operation for — including one of this service's own routes under a method its model
/// does not bind, asked for at the very address that serves the read plane.
///
/// The rows differ in the method the route was asked for under; the modelled `POST` on the
/// same route at the same address is served, so the refusals are about what is bound and
/// not about the stream.
#[tokio::test]
async fn a_method_the_model_does_not_bind_is_the_same_unroutable_answer() {
    let harness = Harness::new();
    let control = "127.0.0.1:9297";
    harness.create_stream(control, "method-probe").await;
    let read_plane = harness
        .announced_authority(control, "method-probe", "GET_MEDIA")
        .await;

    for method in ["GET", "PUT", "DELETE", "PATCH"] {
        let response = harness
            .send(
                method,
                Some(&read_plane),
                "/getMedia",
                &read_from_the_start("method-probe"),
            )
            .await;
        assert_eq!(
            status(&response),
            400,
            "case {method:?}: this service's model binds no {method} /getMedia, so the front \
             door answers it; got {} {}",
            status(&response),
            text(&response)
        );
        assert_eq!(
            json(&response),
            unknown_operation_envelope(),
            "case {method:?}: expected the unknown-operation envelope as the whole body, got {:?}",
            text(&response)
        );
    }

    let served = harness
        .call_at(&read_plane, "/getMedia", &read_from_the_start("method-probe"))
        .await;
    assert_eq!(
        status(&served),
        200,
        "case \"POST\": the modelled method on the same route at the same address is served, \
         got {} {}",
        status(&served),
        text(&served)
    );
}
