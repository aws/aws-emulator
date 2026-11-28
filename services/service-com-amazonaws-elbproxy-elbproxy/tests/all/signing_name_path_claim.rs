// SPDX-License-Identifier: CC0-1.0

//! Which requests this service claims, under a signing name it does not have to itself.
//!
//! `elasticloadbalancing` is the SigV4 signing name of three services in this emulator:
//! Elastic Load Balancing and Elastic Load Balancing v2, whose `awsQuery` traffic is every
//! request `POST /`, and this one. A service that claimed the signing name alone would
//! claim their traffic too, and the router refuses a request two services claim as
//! `500 AmbiguousService` rather than serving it by an arbitrary one.
//!
//! The rule under test: this service claims a request whose route key is that signing name
//! AND whose first URI path segment is the one its protocol publishes — `rpcv2Cbor` binds
//! every operation to `/service/<service>/operation/<operation>` — and claims nothing on
//! the root path or under another key.

use std::sync::Arc;

use service_com_amazonaws_elbproxy_elbproxy::EmulatedService;
use smithy_server_runtime::Service;

/// The signing name this service shares with the two Elastic Load Balancing front doors,
/// written out here so the assertion pins the shared key rather than restating a constant
/// of the implementation.
const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// The service under test, on its own engine.
fn service() -> EmulatedService<engine::InMemoryEngine> {
    EmulatedService::new(Arc::new(engine::InMemoryEngine::new()))
}

/// The parts of one request: `method` to `path`, signed for `signing_name`.
fn parts(method: &str, path: &str, signing_name: &str) -> http::request::Parts {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified-in-this-suite"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://localhost{path}"))
        .header("host", "localhost")
        .header("authorization", authorization)
        .body(())
        .expect("build request");
    request.into_parts().0
}

#[test]
fn this_service_claims_its_own_protocol_path_space_under_the_shared_signing_name() {
    let service = service();
    // One row per operation shape the protocol publishes: an operation of this model, and
    // one named nothing in it (the claim is about the PATH SPACE, not about the routing
    // table, which refuses an unknown operation itself).
    let claimed = [
        ("a modelled operation", "/service/ELBProxy/operation/ListProxies"),
        ("an operation this model does not declare", "/service/ELBProxy/operation/Nope"),
    ];
    for (case, path) in claimed {
        assert!(
            service.can_handle(&parts("POST", path, SHARED_SIGNING_NAME)),
            "case {case:?}: this service must claim its own path space, got no claim for \
             {path}"
        );
    }
}

#[test]
fn this_service_claims_neither_the_root_path_nor_another_key() {
    let service = service();
    // (case, method, path, signing name) — the awsQuery space the Elastic Load Balancing
    // front doors own under the SAME key, a path of no shape this model publishes, and
    // this model's own path space under another service's key.
    let unclaimed = [
        (
            "the root path the awsQuery front doors own",
            "POST",
            "/",
            SHARED_SIGNING_NAME,
        ),
        (
            "a path of no shape this model publishes",
            "GET",
            "/proxies",
            SHARED_SIGNING_NAME,
        ),
        (
            "this model's path space under another service's key",
            "POST",
            "/service/ELBProxy/operation/ListProxies",
            "ec2",
        ),
    ];
    for (case, method, path, signing_name) in unclaimed {
        assert!(
            !service.can_handle(&parts(method, path, signing_name)),
            "case {case:?}: this service must leave {method} {path} signed for \
             {signing_name} to whoever owns it"
        );
    }
}
