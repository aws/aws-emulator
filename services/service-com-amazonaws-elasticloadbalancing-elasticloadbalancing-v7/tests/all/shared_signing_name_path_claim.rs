// SPDX-License-Identifier: CC0-1.0

//! WHICH requests this front door claims under the signing name it shares, along the one
//! dimension its sibling suite leaves out: WHERE the request was addressed.
//!
//! `shared_name_front_door_claim.rs` tables this front door's claim over the PATH space of
//! the shared name, every row addressed the same way. This one varies the addressing instead
//! — the ENDPOINT the shared name publishes, whose first label names the service, and a bare
//! emulator address that names no service, the two tiers the router resolves a routing key
//! from — and adds the two signals that suite does not carry: an `X-Amz-Target`, which is no
//! part of the `awsQuery` wire form, and the ELBv2 model's own key on this endpoint's host.
//!
//! RULE: a signing name several services publish cannot decide which of them serves a
//! request, so each of them claims only the requests its OWN wire form names — this front
//! door's form is `awsQuery`, which posts every action to the root path and names the API in
//! the envelope, so the root post on the shared key is its traffic whichever tier resolved
//! that key and whatever else the request carries, while the `rpcv2Cbor` form its peer under
//! the same name names itself in is never its traffic. WHERE the request was addressed then
//! decides the rest: the signing name is this endpoint's prefix, so a request sent to the
//! host that names it reached the endpoint this API is published at and is answered by that
//! API in its own envelope, as the unresolvable form it is; a request that names the service
//! by its credential SCOPE alone, on a path no model here binds, is addressed at no endpoint
//! of it and is claimed by nobody — the router's own `404 UnknownService`, which is no
//! ambiguity.
//!
//! The three-service table at the router layer is
//! `core/router/tests/all/elasticloadbalancing_signing_name_claims.rs`, and the same two
//! verdicts are pinned through a dispatch by
//! `core/router/tests/all/elb_proxy_shares_the_elasticloadbalancing_key.rs` and
//! `core/router/tests/all/elbproxy_shared_signing_name_routing.rs`.

use std::sync::Arc;

use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService;
use smithy_server_runtime::Service;

/// The signing name this front door owns and two other services publish, written out here so
/// a row pins the shared key rather than restating a constant of the implementation.
const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// The host of the ENDPOINT this signing name publishes, which an AWS SDK client of either
/// Elastic Load Balancing API addresses.
const ENDPOINT_HOST: &str = "elasticloadbalancing.us-east-1.amazonaws.com";

/// A bare emulator address, which names no service: a request sent here names the service by
/// its credential scope alone.
const BARE_HOST: &str = "localhost:9297";

/// The service under test, on an engine of its own.
fn service() -> EmulatedService<engine::InMemoryEngine> {
    EmulatedService::new(Arc::new(engine::InMemoryEngine::new()))
}

/// The parts of one request: `POST path` to `host`, signed for `signing_name`, carrying
/// `target` as its `X-Amz-Target` when the row names one.
fn parts(
    host: &str,
    path: &str,
    target: Option<&str>,
    signing_name: &str,
) -> http::request::Parts {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified-in-this-suite"
    );
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization);
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    builder
        .body(())
        .expect("the test request is well formed")
        .into_parts()
        .0
}

/// One row: what it is called, the host it is addressed at, the path, the `X-Amz-Target` it
/// carries, the name it is signed for, and whether this front door claims it.
struct Row {
    case: &'static str,
    host: &'static str,
    path: &'static str,
    target: Option<&'static str>,
    signing_name: &'static str,
    claimed: bool,
}

#[test]
fn this_front_door_claims_its_own_wire_form_wherever_it_was_addressed() {
    let service = service();
    let rows = [
        Row {
            // The `awsQuery` traffic of both Elastic Load Balancing APIs — `POST /` with the
            // action in the form body — named by the credential scope alone, on an address
            // that names no service: the root path IS this model's own wire surface, so where
            // the request was sent makes no difference to the claim.
            case: "the awsQuery root post on a bare emulator endpoint",
            host: BARE_HOST,
            path: "/",
            target: None,
            signing_name: SHARED_SIGNING_NAME,
            claimed: true,
        },
        Row {
            // Still the root post, and still this endpoint's: an `X-Amz-Target` is no part
            // of the `awsQuery` wire form, so it names nothing this claim reads.
            case: "the awsQuery root post carrying a target header",
            host: BARE_HOST,
            path: "/",
            target: Some("SomeOtherService.SomeOperation"),
            signing_name: SHARED_SIGNING_NAME,
            claimed: true,
        },
        Row {
            // A path NO Elastic Load Balancing model binds, addressed at this endpoint's own
            // host: the request reached the endpoint this API is published at, so this API
            // answers it — in its own envelope, as the unresolvable form it is — rather than
            // reaching no service at all.
            case: "a path no model of this name binds, at this endpoint's host",
            host: ENDPOINT_HOST,
            path: "/2015-12-01/load-balancers",
            target: None,
            signing_name: SHARED_SIGNING_NAME,
            claimed: true,
        },
        Row {
            // The same path named by the credential SCOPE alone, on an address that names no
            // service: addressed at no endpoint of this API and on a path no model here
            // binds, so it is claimed by nobody. This is the cell the addressing decides.
            case: "a path no model of this name binds, on a bare emulator endpoint",
            host: BARE_HOST,
            path: "/2015-12-01/load-balancers",
            target: None,
            signing_name: SHARED_SIGNING_NAME,
            claimed: false,
        },
        Row {
            // A form naming a service NO service of this name is, at this endpoint's host:
            // the peer declines it (the form names another service) and the API published at
            // the endpoint the request reached answers it.
            case: "a form naming no service of this name, at this endpoint's host",
            host: ENDPOINT_HOST,
            path: "/service/ELBMirror/operation/ListProxies",
            target: None,
            signing_name: SHARED_SIGNING_NAME,
            claimed: true,
        },
        Row {
            // And by the credential scope alone, where no endpoint of this API was
            // addressed: claimed by nobody.
            case: "a form naming no service of this name, on a bare emulator endpoint",
            host: BARE_HOST,
            path: "/service/ELBMirror/operation/ListProxies",
            target: None,
            signing_name: SHARED_SIGNING_NAME,
            claimed: false,
        },
        Row {
            // The `rpcv2Cbor` peer's own form, at this endpoint's host: it names itself
            // there and claims exactly it, so this front door declines it however the
            // request was addressed.
            case: "the rpcv2Cbor peer naming itself, at this endpoint's host",
            host: ENDPOINT_HOST,
            path: "/service/ELBProxy/operation/ListProxies",
            target: None,
            signing_name: SHARED_SIGNING_NAME,
            claimed: false,
        },
        Row {
            // The ELBv2 model's own key on this endpoint's host: the routing key names that
            // model, which claims its own traffic, so none of this front door's surfaces are
            // addressed at all.
            case: "the ELBv2 model's own key, which that model claims",
            host: ENDPOINT_HOST,
            path: "/",
            target: None,
            signing_name: "elasticloadbalancingv2",
            claimed: false,
        },
    ];

    for row in rows {
        let claimed = service.can_handle(&parts(
            row.host,
            row.path,
            row.target,
            row.signing_name,
        ));
        assert_eq!(
            claimed, row.claimed,
            "case {:?} (POST {} to {}, target {:?}, signed for {}): expected can_handle to \
             answer {}, got {claimed}",
            row.case, row.path, row.host, row.target, row.signing_name, row.claimed
        );
    }
}
