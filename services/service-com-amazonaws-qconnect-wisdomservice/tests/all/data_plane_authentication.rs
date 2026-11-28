// SPDX-License-Identifier: CC0-1.0

//! Where this service's data planes are served from, which is what decides whether a
//! request to one of them is authenticated.
//!
//! Three paths bind no modelled operation — the upload slot a `StartContentUpload` URL
//! names, a content's asset and a message-template attachment's file — so the generated
//! routing table resolves none of them. The router offers a service two seams for such a
//! request, and they differ in exactly one way that matters here:
//!
//! * `override_routing_error` is answered BEFORE the router authenticates the request, so
//!   anything served there serves stored bytes to a request carrying no credential;
//! * `override_operation` is resolved before authentication RUNS, so the router
//!   authenticates the request against the schemes that operation declares and refuses it
//!   before the operation sees it.
//!
//! The rule this suite pins: every data-plane path is served through the second seam and
//! none through the first, and the operation that serves them declares this endpoint's
//! SigV4 scheme — a request to one of those paths names no modelled operation, so that
//! declaration is the only thing that tells the router what to verify. The locators
//! themselves are query-signed, which `content_upload.rs` pins on the wire.

use std::sync::Arc;

use aws_smithy_types::body::SdkBody;
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::Service;

/// A UUID shaped the way the model's `Uuid` requires, for the paths below. The seam reads
/// the PATH alone, so no record has to exist for the question this suite asks.
const KNOWLEDGE_BASE_ID: &str = "11111111-2222-3333-4444-555555555555";
const RESOURCE_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The scheme shape id the model declares for this service
/// (`smithy_query(query="service_info")`: `aws.auth#sigv4`), written out here so the
/// assertion pins the scheme rather than restating a constant of the implementation.
const SIGV4: &str = "aws.auth#sigv4";

/// The service under test, on its own engine.
fn service() -> EmulatedService<engine::InMemoryEngine> {
    EmulatedService::new(Arc::new(engine::InMemoryEngine::new()))
}

/// One request to `path`, in the runtime-API shape the seams are handed.
fn request(method: &str, path: &str) -> aws_smithy_runtime_api::http::Request {
    let built = http::Request::builder()
        .method(method)
        .uri(format!("https://wisdom.us-east-1.amazonaws.com{path}"))
        .body(SdkBody::empty())
        .expect("build request");
    built.try_into().expect("a runtime-api request")
}

/// The three paths this service publishes URLs into, and the method a caller follows each
/// with.
fn data_plane_paths() -> Vec<(&'static str, &'static str, String)> {
    vec![
        (
            "the content-upload slot",
            "PUT",
            format!("/knowledgeBases/{KNOWLEDGE_BASE_ID}/uploads/{RESOURCE_ID}"),
        ),
        (
            "a content's asset",
            "GET",
            format!("/knowledgeBases/{KNOWLEDGE_BASE_ID}/contents/{RESOURCE_ID}/asset"),
        ),
        (
            "a message-template attachment's file",
            "GET",
            format!(
                "/knowledgeBases/{KNOWLEDGE_BASE_ID}/messageTemplates/{RESOURCE_ID}\
                 /attachments/{RESOURCE_ID}/file"
            ),
        ),
    ]
}

#[test]
fn every_data_plane_path_is_served_through_the_seam_the_router_authenticates() {
    let service = service();
    for (case, method, path) in data_plane_paths() {
        let req = request(method, &path);
        let operation = service.override_operation(&req);
        assert!(
            operation.is_some(),
            "case {case:?}: the path must name an operation the router can authenticate, \
             got none for {method} {path}"
        );
        let schemes = operation.expect("the plane operation").auth_schemes();
        assert_eq!(
            schemes,
            Some([SIGV4].as_slice()),
            "case {case:?}: the plane must declare this endpoint's own scheme, since the \
             request names no modelled operation to take one from, got {schemes:?}"
        );
        assert!(
            service.override_routing_error(&req).is_none(),
            "case {case:?}: nothing may be served from the seam the router answers before \
             it authenticates, got an answer for {method} {path}"
        );
    }
}

#[test]
fn a_path_that_is_not_a_data_plane_path_is_left_to_the_routing_table() {
    let service = service();
    // One row per way a path can miss: a modelled route, the modelled route one segment
    // ABOVE each plane (which the planes must not swallow), and a path of neither shape.
    let rows: Vec<(&str, &str, String)> = vec![
        ("the modelled assistant listing", "GET", "/assistants".to_string()),
        (
            "the modelled read of a content",
            "GET",
            format!("/knowledgeBases/{KNOWLEDGE_BASE_ID}/contents/{RESOURCE_ID}"),
        ),
        (
            "the modelled read of an attachment's template",
            "GET",
            format!("/knowledgeBases/{KNOWLEDGE_BASE_ID}/messageTemplates/{RESOURCE_ID}"),
        ),
        (
            "the modelled start of an upload",
            "POST",
            format!("/knowledgeBases/{KNOWLEDGE_BASE_ID}/upload"),
        ),
        (
            "a path of no shape this service publishes",
            "GET",
            format!("/knowledgeBases/{KNOWLEDGE_BASE_ID}/uploads/{RESOURCE_ID}/extra"),
        ),
    ];
    for (case, method, path) in rows {
        let req = request(method, &path);
        assert!(
            service.override_operation(&req).is_none(),
            "case {case:?}: the data plane must not claim {method} {path}"
        );
        assert!(
            service.override_routing_error(&req).is_none(),
            "case {case:?}: the routing-error seam serves nothing at all, got an answer for \
             {method} {path}"
        );
    }
}
