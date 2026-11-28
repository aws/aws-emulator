// SPDX-License-Identifier: CC0-1.0

//! Which requests the query FRONT DOOR of the shared `elasticloadbalancing` signing name
//! claims, and which it leaves to whatever service names itself in them.
//!
//! THE RULE, stated over the signing name rather than over one request: a signing name
//! several services publish cannot decide which of them serves a request, so this front
//! door's claim is bounded by its OWN modelled wire surface — an `awsQuery` endpoint posts
//! every action to the root path and names the API in the envelope
//! (`smithy_query(query="service_info")`: this model publishes `aws.auth#sigv4` name
//! `elasticloadbalancing` with protocol `awsQuery`) — plus a request ADDRESSED TO the
//! endpoint that name is the prefix of, which names no form of its `rpcv2Cbor` peer: the API
//! published there answers that one in its own envelope, as the unresolvable request it is
//! (the claim `src/service.rs::can_handle` states). A request addressed anywhere else on that
//! key is left to the service that names itself there, which for the `rpcv2Cbor` peer of this
//! name is its own `/service/<Service>/operation/<Operation>` path. Nothing on the key may be
//! claimed twice (`500 AmbiguousService`), and a request this front door declines reaches
//! nobody here, which is honestly the router's `404 UnknownService` rather than an answer from
//! an endpoint that does not serve it (the rule `src/service.rs::can_handle` and
//! `service_common::claims_modeled_path` implement).
//!
//! What this suite owns, and what it does NOT: the ROUTER-level tables with every service
//! of the shared name mounted together live in `core/router/tests/all/` — the two query
//! API versions in `elb_classic_and_v2_signing_name_routing.rs` and the family's claims in
//! `elb_signing_name_family_claims.rs`. This suite drives THIS front door alone, which is
//! what makes both halves of the rule visible separately: a request it claims is answered
//! here in this endpoint's own `<ErrorResponse>` envelope, and a request it declines
//! reaches nobody here.
//!
//! The harness (`emulator`) is the one `wire.rs` owns.

use ::bytes::Bytes;
use smithy_server_runtime::{http, MultiServiceRouter};

use crate::wire::emulator;

/// The region every row below is signed for.
const REGION: &str = "eu-west-1";

/// What one row expects of this front door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Claim {
    /// This endpoint's own wire surface, or a request addressed to this endpoint that names
    /// no form of it: it answers in its own awsQuery envelope.
    FrontDoor,
    /// Not this endpoint's traffic, so with nothing else mounted it reaches NOBODY.
    Elsewhere,
}

/// One row: the signing name the request is signed for, the path it is addressed at, and
/// whose request it is.
struct Row {
    case: &'static str,
    signing_name: &'static str,
    path: &'static str,
    claim: Claim,
}

/// The table.
///
/// The rows differ along what the rule generalises over: the root path the query wire
/// uses, the `rpcv2Cbor` path space the peer of this signing name names itself in, a REST
/// path no Elastic Load Balancing model binds on this endpoint and the same path on
/// another service's endpoint, and the root path signed for a key that is not this
/// endpoint's at all.
const ROWS: &[Row] = &[
    Row {
        case: "the query wire's own root path",
        signing_name: "elasticloadbalancing",
        path: "/",
        claim: Claim::FrontDoor,
    },
    Row {
        case: "the rpcv2Cbor path space of the peer on this signing name",
        signing_name: "elasticloadbalancing",
        path: "/service/ELBProxy/operation/ListProxies",
        claim: Claim::Elsewhere,
    },
    Row {
        // Addressed AT this endpoint — the host's first label is this signing name — on a
        // path no model of this API binds and no form of its `rpcv2Cbor` peer: the API
        // published at the endpoint the request reached is what answers it, in its own
        // envelope, as the unresolvable request it is.
        case: "a REST path no Elastic Load Balancing model binds",
        signing_name: "elasticloadbalancing",
        path: "/2012-06-01/load-balancers",
        claim: Claim::FrontDoor,
    },
    Row {
        // The same path addressed to ANOTHER service's endpoint: that host names no
        // Elastic Load Balancing endpoint and the path is no form of this wire, so this
        // front door claims nothing of it.
        case: "a REST path on another service's endpoint",
        signing_name: "sqs",
        path: "/2012-06-01/load-balancers",
        claim: Claim::Elsewhere,
    },
    Row {
        case: "the root path signed for another service's key",
        signing_name: "sqs",
        path: "/",
        claim: Claim::Elsewhere,
    },
];

/// POST `path`, signed for `signing_name`, with a body that names no awsQuery action — so
/// what the answer shows is WHICH endpoint took the request, not what an action does.
async fn addressed(
    router: &MultiServiceRouter,
    signing_name: &str,
    path: &str,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri(path)
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::new())
        .expect("a well-formed request")
        ;
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body().as_ref()).to_string(),
    )
}

/// Every row reaches exactly one place: this endpoint's own envelope for its own wire
/// surface, and nobody at all for a request addressed outside it.
#[tokio::test]
async fn the_front_door_claims_its_own_wire_surface_and_leaves_the_rest() {
    let router = emulator();

    for row in ROWS {
        let (status, answered) = addressed(&router, row.signing_name, row.path).await;
        assert!(
            !answered.contains("AmbiguousService"),
            "case {:?}: one request keeps one claimant, got {status} with {answered}",
            row.case
        );

        match row.claim {
            Claim::FrontDoor => {
                assert!(
                    !answered.contains("UnknownService"),
                    "case {:?}: this endpoint owns its own wire surface, got {status} with \
                     {answered}",
                    row.case
                );
                assert!(
                    answered.contains("<ErrorResponse"),
                    "case {:?}: expected this endpoint's own awsQuery envelope, got {status} \
                     with {answered}",
                    row.case
                );
                assert_eq!(
                    status, 400,
                    "case {:?}: a request naming no action is refused, got {status} with \
                     {answered}",
                    row.case
                );
            }
            Claim::Elsewhere => {
                assert!(
                    answered.contains("UnknownService"),
                    "case {:?}: expected the request to be left to the service that names \
                     itself in it, got {status} with {answered}",
                    row.case
                );
                assert_eq!(
                    status, 404,
                    "case {:?}: a request no mounted service claims is the router's 404, got \
                     {status} with {answered}",
                    row.case
                );
            }
        }
    }
}
